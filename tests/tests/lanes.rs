//! End-to-end tests for LaneLedger against a real AgentDirectory, executed in-process by the Ootle
//! engine (`tari_template_test_tooling`). The directory template is built from a sibling checkout of
//! github.com/alex3000-ai/ootle-agent-directory (see `DIRECTORY_SRC`).

use agent_directory::{register_message, retire_message, rotate_message, DIRECTORY_DOMAIN};
use lanes_ledger::{lane_voucher_message, LANE_VOUCHER_DOMAIN, LISTING_MAX};
use ootle_byte_type::ToByteType;
use tari_crypto::{
    keys::{PublicKey as _, SecretKey as _},
    ristretto::{RistrettoPublicKey, RistrettoSchnorr, RistrettoSecretKey},
};
use tari_engine_types::virtual_substate::{VirtualSubstate, VirtualSubstateId};
use tari_ootle_common_types::RistrettoSchnorrBlake2bVerifier;
use tari_ootle_transaction::{args, builder::named_args::NamedArg, Transaction};
use tari_template_lib_types::{
    constants::TARI_TOKEN,
    crypto::{RistrettoPublicKeyBytes, SignaturePayload},
    Amount, ComponentAddress, NonFungibleAddress, ResourceAddress,
};
use tari_template_test_tooling::TemplateTest;

/// The AgentDirectory template source, relative to this crate.
const DIRECTORY_SRC: &str = "../../ootle-agent-directory/directory";
const NAME_FEE: u64 = 1_000;
const LANE_CLOSE_EPOCHS: u64 = 6;

struct Party {
    account: ComponentAddress,
    proof: NonFungibleAddress,
    key: RistrettoSecretKey,
}

impl Party {
    fn public_key(&self) -> RistrettoPublicKeyBytes {
        pk(&self.key)
    }
}

/// A directory agent acting as a provider.
struct Provider {
    party: Party,
    owner: RistrettoSecretKey,
    name: String,
}

fn pk(key: &RistrettoSecretKey) -> RistrettoPublicKeyBytes {
    RistrettoPublicKey::from_secret_key(key).to_byte_type()
}

fn funded(test: &mut TemplateTest) -> Party {
    let (account, proof, key) = test.create_funded_account();
    Party { account, proof, key }
}

fn schnorr(domain: &[u8], key: &RistrettoSecretKey, message: &[u8]) -> SignaturePayload {
    let (nonce, public_nonce) = RistrettoPublicKey::random_keypair(&mut rand::rng());
    let challenge = RistrettoSchnorrBlake2bVerifier::compute_challenge(domain, message, &pk(key), &public_nonce.to_byte_type());
    SignaturePayload::RistrettoSchnorrBlake2b(RistrettoSchnorr::sign_raw_uniform(key, nonce, &challenge).expect("sign").to_byte_type())
}

/// Signs a lane voucher exactly as a buyer's client would.
fn sign_voucher(key: &RistrettoSecretKey, ledger: &ComponentAddress, lane_id: u64, spent_cum: Amount) -> SignaturePayload {
    schnorr(LANE_VOUCHER_DOMAIN, key, &lane_voucher_message(ledger, lane_id, spent_cum))
}

struct Network {
    test: TemplateTest,
    directory: ComponentAddress,
    ledger: ComponentAddress,
    reader: Party,
}

impl Network {
    /// Deploys a directory and a ledger over it.
    fn deploy() -> Self {
        let mut test = TemplateTest::new(env!("CARGO_MANIFEST_DIR"), [DIRECTORY_SRC, "../ledger"]);
        let reader = funded(&mut test);

        let directory_template = test.get_template_address("AgentDirectory");
        test.execute_expect_success(
            test.transaction().call_function(directory_template, "new", args![TARI_TOKEN, Amount::from(NAME_FEE)]).finish().seal(&reader.key),
            vec![reader.proof.clone()],
        );
        let directory = *test.return_values()[0].component_addresses().first().expect("directory");

        let ledger_template = test.get_template_address("LaneLedger");
        test.execute_expect_success(
            test.transaction()
                .call_function(ledger_template, "new", args![directory, TARI_TOKEN, LANE_CLOSE_EPOCHS])
                .finish()
                .seal(&reader.key),
            vec![reader.proof.clone()],
        );
        let ledger = *test.return_values()[0].component_addresses().first().expect("ledger");
        Self { test, directory, ledger, reader }
    }

    /// Registers `name` in the directory with a fresh account and owner key.
    fn provider(&mut self, name: &str) -> Provider {
        let party = funded(&mut self.test);
        let owner = RistrettoSecretKey::random(&mut rand::rng());
        let sig = schnorr(DIRECTORY_DOMAIN, &owner, &register_message(&self.directory, name, &party.public_key(), ""));
        let tx = self
            .test
            .transaction()
            .call_method(party.account, "withdraw", args![TARI_TOKEN, NAME_FEE])
            .put_last_instruction_output_on_workspace("fee")
            .call_method(self.directory, "register", args![name, pk(&owner), "", "", sig, Workspace("fee")])
            .put_last_instruction_output_on_workspace("card")
            .call_method(party.account, "deposit", args![Workspace("card")])
            .finish()
            .seal(&party.key);
        self.test.execute_expect_success(tx, vec![party.proof.clone()]);
        Provider { party, owner, name: name.to_string() }
    }

    /// The owner replaces the provider's agent key; the provider keeps its account.
    fn rotate(&mut self, provider: &mut Provider, nonce: u64) {
        let new_key = RistrettoSecretKey::random(&mut rand::rng());
        let sig = schnorr(DIRECTORY_DOMAIN, &provider.owner, &rotate_message(&self.directory, &provider.name, nonce, &pk(&new_key)));
        let tx = self
            .test
            .transaction()
            .call_method(self.directory, "rotate_agent_key", args![provider.name.as_str(), pk(&new_key), sig])
            .finish()
            .seal(&provider.party.key);
        self.test.execute_expect_success(tx, vec![provider.party.proof.clone()]);
        provider.party.key = new_key;
    }

    fn retire(&mut self, provider: &Provider, nonce: u64) {
        let sig = schnorr(DIRECTORY_DOMAIN, &provider.owner, &retire_message(&self.directory, &provider.name, nonce));
        let tx = self.test.transaction().call_method(self.directory, "retire", args![provider.name.as_str(), sig]).finish().seal(&provider.party.key);
        self.test.execute_expect_success(tx, vec![provider.party.proof.clone()]);
    }

    fn deposit(&mut self, buyer: &Party, amount: u64) {
        let transaction = self
            .test
            .transaction()
            .call_method(buyer.account, "withdraw", args![TARI_TOKEN, amount])
            .put_last_instruction_output_on_workspace("funds")
            .call_method(self.ledger, "deposit", args![Workspace("funds")])
            .finish()
            .seal(&buyer.key);
        self.test.execute_expect_success(transaction, vec![buyer.proof.clone()]);
    }

    fn open_lane(&mut self, buyer: &Party, provider: &Provider, amount: u64) -> u64 {
        self.signed_call(buyer, "open_lane", args![provider.name.as_str(), Amount::from(amount), buyer.public_key()]);
        self.test.return_values()[0].decoded::<u64>().expect("lane id")
    }

    fn claim_transaction(&self, claimer: &Party, lane_id: u64, spent_cum: Amount, signature: SignaturePayload) -> Transaction {
        self.test
            .transaction()
            .call_method(self.ledger, "claim", args![lane_id, spent_cum, signature])
            .put_last_instruction_output_on_workspace("payout")
            .call_method(claimer.account, "deposit", args![Workspace("payout")])
            .finish()
            .seal(&claimer.key)
    }

    fn claim(&mut self, provider: &Party, buyer: &Party, lane_id: u64, spent_cum: u64) {
        let spent_cum = Amount::from(spent_cum);
        let signature = sign_voucher(&buyer.key, &self.ledger, lane_id, spent_cum);
        let transaction = self.claim_transaction(provider, lane_id, spent_cum, signature);
        self.test.execute_expect_success(transaction, vec![provider.proof.clone()]);
    }

    fn signed_call(&mut self, party: &Party, method: &str, args: Vec<NamedArg>) {
        let transaction = self.test.transaction().call_method(self.ledger, method, args).finish().seal(&party.key);
        self.test.execute_expect_success(transaction, vec![party.proof.clone()]);
    }

    fn signed_call_fails(&mut self, party: &Party, method: &str, args: Vec<NamedArg>, expected: &str) {
        let transaction = self.test.transaction().call_method(self.ledger, method, args).finish().seal(&party.key);
        self.rejected(transaction, vec![party.proof.clone()], expected);
    }

    /// Asserts the transaction is rejected for the expected reason, not just that it fails.
    fn rejected(&mut self, transaction: Transaction, proofs: Vec<NonFungibleAddress>, expected: &str) {
        let reason = self.test.execute_expect_failure(transaction, proofs).to_string();
        assert!(reason.contains(expected), "expected rejection containing {expected:?}, got: {reason}");
    }

    fn listing(&mut self, name: &str) -> Option<String> {
        self.test.call_method(self.ledger, "listing", args![name], vec![])
    }

    fn balance(&mut self, party: &Party) -> Amount {
        self.test.call_method(self.ledger, "balance", args![party.public_key()], vec![])
    }

    /// `Lane` only implements CBOR decoding, so read it from a transaction's return value rather
    /// than through `call_method` (which also requires serde).
    fn lane(&mut self, lane_id: u64) -> lanes_ledger::Lane {
        let transaction = self.test.transaction().call_method(self.ledger, "lane", args![lane_id]).finish().seal(&self.reader.key);
        self.test.execute_expect_success(transaction, vec![self.reader.proof.clone()]);
        let lane: Option<lanes_ledger::Lane> = self.test.return_values()[0].decoded().expect("decode lane");
        lane.expect("lane exists")
    }

    fn account_balance(&mut self, party: &Party) -> Amount {
        self.test.call_method(party.account, "balance", args![TARI_TOKEN], vec![])
    }

    fn set_epoch(&mut self, epoch: u64) {
        self.test.set_virtual_substate(VirtualSubstateId::CurrentEpoch, VirtualSubstate::CurrentEpoch(epoch));
    }

    /// vault == buyer balances + unclaimed escrow in open lanes.
    fn assert_conservation(&mut self, buyers: &[&Party], lanes: &[u64]) {
        let vault: Amount = self.test.call_method(self.ledger, "vault_balance", args![], vec![]);
        let mut expected = Amount::zero();
        for buyer in buyers {
            expected = expected.checked_add(self.balance(buyer)).unwrap();
        }
        for lane_id in lanes {
            let lane = self.lane(*lane_id);
            if !lane.closed {
                expected = expected.checked_add(lane.deposited.checked_sub(lane.claimed).unwrap()).unwrap();
            }
        }
        assert_eq!(vault, expected, "ledger accounting does not conserve funds");
    }
}

// ---- no admin ----

#[test]
fn settings_are_fixed_at_creation_and_nothing_is_admin_gated() {
    let mut network = Network::deploy();
    let config: (ComponentAddress, ResourceAddress, u64) = network.test.call_method(network.ledger, "config", args![], vec![]);
    assert_eq!(config, (network.directory, TARI_TOKEN, LANE_CLOSE_EPOCHS));

    // The old admin surface is gone: no fees, no pause, no parameter changes.
    for method in ["withdraw_fees", "set_params", "set_paused", "fees_owed"] {
        let tx = network.test.transaction().call_method(network.ledger, method, args![]).finish().seal(&network.reader.key);
        network.rejected(tx, vec![network.reader.proof.clone()], "");
    }

    let template = network.test.get_template_address("LaneLedger");
    let tx = network
        .test
        .transaction()
        .call_function(template, "new", args![network.directory, TARI_TOKEN, 0u64])
        .finish()
        .seal(&network.reader.key);
    network.rejected(tx, vec![network.reader.proof.clone()], "close window must be at least one epoch");
}

// ---- listings ----

#[test]
fn only_the_agents_current_key_sets_its_listing() {
    let mut network = Network::deploy();
    let mut alice = network.provider("alice");
    let stranger = funded(&mut network.test);
    let url = "https://ootlebook.example/services/summaries";

    network.signed_call(&alice.party, "set_listing", args!["alice", url]);
    assert_eq!(network.listing("alice").as_deref(), Some(url));
    network.signed_call_fails(&stranger, "set_listing", args!["alice", "https://evil.example"], "only the agent's current key can do this");
    network.signed_call_fails(&alice.party, "set_listing", args!["alice", "x".repeat(LISTING_MAX + 1)], "listing is too long");

    // After a key rotation only the new key counts.
    let old = Party { account: alice.party.account, proof: alice.party.proof.clone(), key: alice.party.key.clone() };
    network.rotate(&mut alice, 0);
    network.signed_call_fails(&old, "set_listing", args!["alice", "https://old-key.example"], "only the agent's current key can do this");
    network.signed_call(&alice.party, "set_listing", args!["alice", ""]);
    assert_eq!(network.listing("alice"), None);

    network.retire(&alice, 1);
    network.signed_call_fails(&alice.party, "set_listing", args!["alice", url], "that agent is retired");
}

// ---- lanes ----

#[test]
fn lane_claims_pay_the_provider_in_full() {
    let mut network = Network::deploy();
    let provider = network.provider("alice");
    let buyer = funded(&mut network.test);

    network.deposit(&buyer, 10_000);
    let lane_id = network.open_lane(&buyer, &provider, 6_000);
    assert_eq!(network.balance(&buyer), Amount::from(4_000u64));
    let lane = network.lane(lane_id);
    assert_eq!((lane.provider.as_str(), lane.provider_key), ("alice", provider.party.public_key()));
    network.assert_conservation(&[&buyer], &[lane_id]);

    let before = network.account_balance(&provider.party);
    network.claim(&provider.party, &buyer, lane_id, 1_000);
    assert_eq!(network.account_balance(&provider.party).checked_sub(before).unwrap(), Amount::from(1_000u64));

    // A later voucher pays only the new part.
    network.claim(&provider.party, &buyer, lane_id, 2_500);
    assert_eq!(network.account_balance(&provider.party).checked_sub(before).unwrap(), Amount::from(2_500u64));
    assert_eq!(network.lane(lane_id).claimed, Amount::from(2_500u64));
    network.assert_conservation(&[&buyer], &[lane_id]);
}

#[test]
fn claims_reject_bad_vouchers() {
    let mut network = Network::deploy();
    let provider = network.provider("alice");
    let buyer = funded(&mut network.test);
    let stranger = funded(&mut network.test);
    network.deposit(&buyer, 10_000);
    let lane_id = network.open_lane(&buyer, &provider, 5_000);
    let p = &provider.party;

    // Signed by someone other than the lane's voucher key.
    let forged = sign_voucher(&stranger.key, &network.ledger, lane_id, Amount::from(1_000u64));
    let transaction = network.claim_transaction(p, lane_id, Amount::from(1_000u64), forged);
    network.rejected(transaction, vec![p.proof.clone()], "invalid voucher signature");

    // A genuine signature over a different amount.
    let other_amount = sign_voucher(&buyer.key, &network.ledger, lane_id, Amount::from(900u64));
    let transaction = network.claim_transaction(p, lane_id, Amount::from(1_000u64), other_amount);
    network.rejected(transaction, vec![p.proof.clone()], "invalid voucher signature");

    // More than the lane holds, even though the buyer signed it.
    let too_much = sign_voucher(&buyer.key, &network.ledger, lane_id, Amount::from(5_001u64));
    let transaction = network.claim_transaction(p, lane_id, Amount::from(5_001u64), too_much);
    network.rejected(transaction, vec![p.proof.clone()], "voucher exceeds the lane's escrow");

    // Only the lane's provider may claim.
    let valid = sign_voucher(&buyer.key, &network.ledger, lane_id, Amount::from(1_000u64));
    let transaction = network.claim_transaction(&stranger, lane_id, Amount::from(1_000u64), valid.clone());
    network.rejected(transaction, vec![stranger.proof.clone()], "only the lane's provider can claim");

    // The valid voucher works once; replaying it pays nothing more.
    let transaction = network.claim_transaction(p, lane_id, Amount::from(1_000u64), valid.clone());
    network.test.execute_expect_success(transaction, vec![p.proof.clone()]);
    let transaction = network.claim_transaction(p, lane_id, Amount::from(1_000u64), valid);
    network.rejected(transaction, vec![p.proof.clone()], "voucher does not exceed the amount already claimed");
    network.assert_conservation(&[&buyer], &[lane_id]);
}

#[test]
fn only_the_lanes_voucher_key_can_sign_vouchers() {
    let mut network = Network::deploy();
    let provider = network.provider("alice");
    let buyer = funded(&mut network.test);
    network.deposit(&buyer, 10_000);

    // The buyer keeps its wallet key in a wallet daemon and signs vouchers with a separate hot key.
    let voucher_secret = RistrettoSecretKey::random(&mut rand::rng());
    network.signed_call(&buyer, "open_lane", args!["alice", Amount::from(5_000u64), pk(&voucher_secret)]);
    let lane_id = network.test.return_values()[0].decoded::<u64>().expect("lane id");
    assert_eq!(network.lane(lane_id).voucher_key, pk(&voucher_secret));

    let spent_cum = Amount::from(1_000u64);
    let by_wallet_key = sign_voucher(&buyer.key, &network.ledger, lane_id, spent_cum);
    let transaction = network.claim_transaction(&provider.party, lane_id, spent_cum, by_wallet_key);
    network.rejected(transaction, vec![provider.party.proof.clone()], "invalid voucher signature");

    let by_voucher_key = sign_voucher(&voucher_secret, &network.ledger, lane_id, spent_cum);
    let transaction = network.claim_transaction(&provider.party, lane_id, spent_cum, by_voucher_key);
    network.test.execute_expect_success(transaction, vec![provider.party.proof.clone()]);
    network.assert_conservation(&[&buyer], &[lane_id]);
}

#[test]
fn lanes_need_an_active_directory_agent() {
    let mut network = Network::deploy();
    let alice = network.provider("alice");
    let buyer = funded(&mut network.test);
    network.deposit(&buyer, 10_000);
    let amount = Amount::from(100u64);

    network.signed_call_fails(&buyer, "open_lane", args!["nobody", amount, buyer.public_key()], "");
    network.signed_call_fails(&buyer, "open_lane", args!["alice", Amount::from(10_001u64), buyer.public_key()], "insufficient balance");
    network.signed_call_fails(&buyer, "open_lane", args!["alice", Amount::zero(), buyer.public_key()], "lane amount must be positive");

    // An agent can't open a lane to itself.
    network.deposit(&alice.party, 1_000);
    network.signed_call_fails(&alice.party, "open_lane", args!["alice", amount, alice.party.public_key()], "cannot open a lane to yourself");

    network.retire(&alice, 0);
    network.signed_call_fails(&buyer, "open_lane", args!["alice", amount, buyer.public_key()], "that agent is retired");
}

#[test]
fn a_lane_stays_with_the_key_it_was_opened_to() {
    let mut network = Network::deploy();
    let mut alice = network.provider("alice");
    let buyer = funded(&mut network.test);
    network.deposit(&buyer, 10_000);
    let old_lane = network.open_lane(&buyer, &alice, 3_000);
    let old = Party { account: alice.party.account, proof: alice.party.proof.clone(), key: alice.party.key.clone() };

    network.rotate(&mut alice, 0);

    // The old lane pays the key it was opened to; the new key can't touch it.
    let spent = Amount::from(500u64);
    let voucher = sign_voucher(&buyer.key, &network.ledger, old_lane, spent);
    let transaction = network.claim_transaction(&alice.party, old_lane, spent, voucher.clone());
    network.rejected(transaction, vec![alice.party.proof.clone()], "only the lane's provider can claim");
    let transaction = network.claim_transaction(&old, old_lane, spent, voucher);
    network.test.execute_expect_success(transaction, vec![old.proof.clone()]);

    // New lanes go to the new key.
    let new_lane = network.open_lane(&buyer, &alice, 1_000);
    assert_eq!(network.lane(new_lane).provider_key, alice.party.public_key());
    network.claim(&alice.party, &buyer, new_lane, 200);
    network.assert_conservation(&[&buyer], &[old_lane, new_lane]);
}

#[test]
fn closing_a_lane_waits_for_the_window_and_returns_the_remainder() {
    let mut network = Network::deploy();
    let provider = network.provider("alice");
    let buyer = funded(&mut network.test);
    network.deposit(&buyer, 10_000);
    let lane_id = network.open_lane(&buyer, &provider, 5_000);
    network.claim(&provider.party, &buyer, lane_id, 1_000);

    network.set_epoch(10);
    network.signed_call(&buyer, "request_close", args![lane_id]);
    network.signed_call_fails(&buyer, "finalize_close", args![lane_id], "close window has not passed");
    network.signed_call_fails(&buyer, "fund_lane", args![lane_id, Amount::from(100u64)], "lane is closing or closed");

    // The provider can still claim its latest voucher inside the window.
    network.set_epoch(15);
    network.claim(&provider.party, &buyer, lane_id, 1_800);
    network.signed_call_fails(&buyer, "finalize_close", args![lane_id], "close window has not passed");

    // Anyone can finalize once the window has passed.
    network.set_epoch(16);
    let anyone = funded(&mut network.test);
    network.signed_call(&anyone, "finalize_close", args![lane_id]);
    assert!(network.lane(lane_id).closed);
    assert_eq!(network.balance(&buyer), Amount::from(5_000u64 + 3_200));
    network.assert_conservation(&[&buyer], &[lane_id]);

    // Nothing more can be claimed from a closed lane.
    let late = sign_voucher(&buyer.key, &network.ledger, lane_id, Amount::from(2_000u64));
    let transaction = network.claim_transaction(&provider.party, lane_id, Amount::from(2_000u64), late);
    network.rejected(transaction, vec![provider.party.proof.clone()], "lane is closed");
}

#[test]
fn buyers_top_up_lanes_and_providers_release_them_early() {
    let mut network = Network::deploy();
    let provider = network.provider("alice");
    let buyer = funded(&mut network.test);
    let stranger = funded(&mut network.test);
    network.deposit(&buyer, 10_000);
    let lane_id = network.open_lane(&buyer, &provider, 5_000);
    network.signed_call(&buyer, "fund_lane", args![lane_id, Amount::from(1_000u64)]);
    network.signed_call_fails(&stranger, "fund_lane", args![lane_id, Amount::from(1u64)], "only the lane's buyer can fund it");
    assert_eq!(network.lane(lane_id).deposited, Amount::from(6_000u64));
    network.claim(&provider.party, &buyer, lane_id, 400);

    network.signed_call_fails(&buyer, "release_lane", args![lane_id], "only the lane's provider can release it");
    network.signed_call(&provider.party, "release_lane", args![lane_id]);
    assert_eq!(network.balance(&buyer), Amount::from(4_000u64 + 5_600));
    network.assert_conservation(&[&buyer], &[lane_id]);
}

#[test]
fn buyers_withdraw_unallocated_funds() {
    let mut network = Network::deploy();
    let provider = network.provider("alice");
    let buyer = funded(&mut network.test);
    network.deposit(&buyer, 10_000);
    let lane_id = network.open_lane(&buyer, &provider, 5_000);
    network.claim(&provider.party, &buyer, lane_id, 2_000);

    network.signed_call_fails(&buyer, "withdraw", args![Amount::from(5_001u64)], "insufficient balance");
    let before = network.account_balance(&buyer);
    let transaction = network
        .test
        .transaction()
        .call_method(network.ledger, "withdraw", args![Amount::from(5_000u64)])
        .put_last_instruction_output_on_workspace("refund")
        .call_method(buyer.account, "deposit", args![Workspace("refund")])
        .finish()
        .seal(&buyer.key);
    network.test.execute_expect_success(transaction, vec![buyer.proof.clone()]);
    assert_eq!(network.balance(&buyer), Amount::zero());
    assert!(network.account_balance(&buyer) > before);
    network.assert_conservation(&[&buyer], &[lane_id]);
}
