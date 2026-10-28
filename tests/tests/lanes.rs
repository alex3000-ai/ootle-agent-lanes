//! End-to-end tests for LaneLedger against a real AgentDirectory, executed in-process by the Ootle
//! engine (`tari_template_test_tooling`). The directory template is built from a sibling checkout of
//! github.com/alex3000-ai/ootle-agent-directory (see `DIRECTORY_SRC`).

use agent_directory::{register_message, retire_message, rotate_message, DIRECTORY_DOMAIN};
use lanes_ledger::{lane_voucher_message, LaneInfo, LaneState, LANE_VOUCHER_DOMAIN, PAGE_SIZE};
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

    /// Builds a transaction from `buyer`'s account that withdraws `amount` into the workspace as "funds" and
    /// calls `method` with `args` followed by the funds.
    fn funded_call(&self, buyer: &Party, amount: u64, method: &str, mut args: Vec<NamedArg>) -> Transaction {
        args.push(NamedArg::Workspace("funds".into()));
        self.test
            .transaction()
            .call_method(buyer.account, "withdraw", args![TARI_TOKEN, amount])
            .put_last_instruction_output_on_workspace("funds")
            .call_method(self.ledger, method, args)
            .finish()
            .seal(&buyer.key)
    }

    fn open_lane(&mut self, buyer: &Party, provider: &Provider, amount: u64) -> u64 {
        let tx = self.funded_call(buyer, amount, "open_lane", args![provider.name.as_str(), buyer.public_key()]);
        self.test.execute_expect_success(tx, vec![buyer.proof.clone()]);
        self.test.return_values().last().unwrap().decoded::<u64>().expect("lane id")
    }

    fn open_lane_fails(&mut self, buyer: &Party, provider: &str, amount: u64, expected: &str) {
        let tx = self.funded_call(buyer, amount, "open_lane", args![provider, buyer.public_key()]);
        self.rejected(tx, vec![buyer.proof.clone()], expected);
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

    fn refund_transaction(&self, buyer: &Party, lane_id: u64) -> Transaction {
        self.test
            .transaction()
            .call_method(self.ledger, "refund", args![lane_id])
            .put_last_instruction_output_on_workspace("refund")
            .call_method(buyer.account, "deposit", args![Workspace("refund")])
            .finish()
            .seal(&buyer.key)
    }

    /// Refunds the lane and returns what came back to the buyer's account.
    fn refund(&mut self, buyer: &Party, lane_id: u64) -> Amount {
        let before = self.account_balance(buyer);
        let tx = self.refund_transaction(buyer, lane_id);
        self.test.execute_expect_success(tx, vec![buyer.proof.clone()]);
        // The buyer's account also paid the transaction fee, which the test engine doesn't charge.
        self.account_balance(buyer).checked_sub(before).unwrap()
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

    /// The lane types only implement CBOR decoding, so read them from a transaction's return value rather
    /// than through `call_method` (which also requires serde).
    fn lane(&mut self, lane_id: u64) -> (LaneInfo, LaneState) {
        let transaction = self.test.transaction().call_method(self.ledger, "lane", args![lane_id]).finish().seal(&self.reader.key);
        self.test.execute_expect_success(transaction, vec![self.reader.proof.clone()]);
        self.test.return_values()[0].decoded().expect("decode lane")
    }

    fn state(&mut self, lane_id: u64) -> LaneState {
        self.lane(lane_id).1
    }

    fn config(&mut self) -> (ComponentAddress, ResourceAddress, u64, ResourceAddress, u64) {
        let transaction = self.test.transaction().call_method(self.ledger, "config", args![]).finish().seal(&self.reader.key);
        self.test.execute_expect_success(transaction, vec![self.reader.proof.clone()]);
        self.test.return_values()[0].decoded().expect("decode config")
    }

    fn account_balance(&mut self, party: &Party) -> Amount {
        self.test.call_method(party.account, "balance", args![TARI_TOKEN], vec![])
    }

    fn set_epoch(&mut self, epoch: u64) {
        self.test.set_virtual_substate(VirtualSubstateId::CurrentEpoch, VirtualSubstate::CurrentEpoch(epoch));
    }

    /// escrow == what every lane still holds.
    fn assert_conservation(&mut self, lanes: &[u64]) {
        let escrow: Amount = self.test.call_method(self.ledger, "escrow", args![], vec![]);
        let mut expected = Amount::zero();
        for lane_id in lanes {
            expected = expected.checked_add(self.state(*lane_id).remaining()).unwrap();
        }
        assert_eq!(escrow, expected, "ledger accounting does not conserve funds");
    }
}

// ---- no admin ----

#[test]
fn settings_are_fixed_at_creation_and_nothing_is_admin_gated() {
    let mut network = Network::deploy();
    let (directory, payment, window, _lanes, count) = network.config();
    assert_eq!((directory, payment, window, count), (network.directory, TARI_TOKEN, LANE_CLOSE_EPOCHS, 0));

    // No admin surface, and v0.2's buyer balances and listings are gone.
    for method in ["withdraw_fees", "set_params", "set_paused", "deposit", "withdraw", "balance", "set_listing", "finalize_close"] {
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

// ---- lanes ----

#[test]
fn lane_claims_pay_the_provider_in_full() {
    let mut network = Network::deploy();
    let provider = network.provider("alice");
    let buyer = funded(&mut network.test);

    let lane_id = network.open_lane(&buyer, &provider, 6_000);
    assert_eq!(lane_id, 1);
    let (info, state) = network.lane(lane_id);
    assert_eq!((info.id, info.buyer, info.provider.as_str(), info.provider_key), (1, buyer.public_key(), "alice", provider.party.public_key()));
    assert_eq!(state.deposited, Amount::from(6_000u64));
    assert_eq!(network.config().4, 1);
    network.assert_conservation(&[lane_id]);

    let before = network.account_balance(&provider.party);
    network.claim(&provider.party, &buyer, lane_id, 1_000);
    assert_eq!(network.account_balance(&provider.party).checked_sub(before).unwrap(), Amount::from(1_000u64));

    // A later voucher pays only the new part.
    network.claim(&provider.party, &buyer, lane_id, 2_500);
    assert_eq!(network.account_balance(&provider.party).checked_sub(before).unwrap(), Amount::from(2_500u64));
    assert_eq!(network.state(lane_id).claimed, Amount::from(2_500u64));
    network.assert_conservation(&[lane_id]);
}

#[test]
fn claims_reject_bad_vouchers() {
    let mut network = Network::deploy();
    let provider = network.provider("alice");
    let buyer = funded(&mut network.test);
    let stranger = funded(&mut network.test);
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

    // Only the lane's provider may claim, and only lanes that exist.
    let valid = sign_voucher(&buyer.key, &network.ledger, lane_id, Amount::from(1_000u64));
    let transaction = network.claim_transaction(&stranger, lane_id, Amount::from(1_000u64), valid.clone());
    network.rejected(transaction, vec![stranger.proof.clone()], "only the lane's provider can claim");
    let transaction = network.claim_transaction(p, 2, Amount::from(1_000u64), valid.clone());
    network.rejected(transaction, vec![p.proof.clone()], "unknown lane");

    // The valid voucher works once; replaying it pays nothing more.
    let transaction = network.claim_transaction(p, lane_id, Amount::from(1_000u64), valid.clone());
    network.test.execute_expect_success(transaction, vec![p.proof.clone()]);
    let transaction = network.claim_transaction(p, lane_id, Amount::from(1_000u64), valid);
    network.rejected(transaction, vec![p.proof.clone()], "voucher does not exceed the amount already claimed");
    network.assert_conservation(&[lane_id]);
}

#[test]
fn only_the_lanes_voucher_key_can_sign_vouchers() {
    let mut network = Network::deploy();
    let provider = network.provider("alice");
    let buyer = funded(&mut network.test);

    // The buyer keeps its wallet key in a wallet daemon and signs vouchers with a separate hot key.
    let voucher_secret = RistrettoSecretKey::random(&mut rand::rng());
    let tx = network.funded_call(&buyer, 5_000, "open_lane", args!["alice", pk(&voucher_secret)]);
    network.test.execute_expect_success(tx, vec![buyer.proof.clone()]);
    let lane_id = network.test.return_values().last().unwrap().decoded::<u64>().expect("lane id");
    assert_eq!(network.lane(lane_id).0.voucher_key, pk(&voucher_secret));

    let spent_cum = Amount::from(1_000u64);
    let by_wallet_key = sign_voucher(&buyer.key, &network.ledger, lane_id, spent_cum);
    let transaction = network.claim_transaction(&provider.party, lane_id, spent_cum, by_wallet_key);
    network.rejected(transaction, vec![provider.party.proof.clone()], "invalid voucher signature");

    let by_voucher_key = sign_voucher(&voucher_secret, &network.ledger, lane_id, spent_cum);
    let transaction = network.claim_transaction(&provider.party, lane_id, spent_cum, by_voucher_key);
    network.test.execute_expect_success(transaction, vec![provider.party.proof.clone()]);
    network.assert_conservation(&[lane_id]);
}

#[test]
fn lanes_need_an_active_directory_agent() {
    let mut network = Network::deploy();
    let alice = network.provider("alice");
    let buyer = funded(&mut network.test);

    network.open_lane_fails(&buyer, "nobody", 100, "");
    network.open_lane_fails(&buyer, "alice", 0, "lane amount must be positive");
    // An agent can't open a lane to itself.
    network.open_lane_fails(&alice.party, "alice", 100, "cannot open a lane to yourself");

    network.retire(&alice, 0);
    network.open_lane_fails(&buyer, "alice", 100, "that agent is retired");
}

#[test]
fn a_lane_stays_with_the_key_it_was_opened_to() {
    let mut network = Network::deploy();
    let mut alice = network.provider("alice");
    let buyer = funded(&mut network.test);
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
    assert_eq!(network.lane(new_lane).0.provider_key, alice.party.public_key());
    network.claim(&alice.party, &buyer, new_lane, 200);
    network.assert_conservation(&[old_lane, new_lane]);
}

#[test]
fn closing_a_lane_waits_for_the_window_and_refunds_the_remainder() {
    let mut network = Network::deploy();
    let provider = network.provider("alice");
    let buyer = funded(&mut network.test);
    let stranger = funded(&mut network.test);
    let lane_id = network.open_lane(&buyer, &provider, 5_000);
    network.claim(&provider.party, &buyer, lane_id, 1_000);

    let tx = network.refund_transaction(&buyer, lane_id);
    network.rejected(tx, vec![buyer.proof.clone()], "close was not requested");
    network.signed_call_fails(&stranger, "request_close", args![lane_id], "only the lane's buyer can close it");

    network.set_epoch(10);
    network.signed_call(&buyer, "request_close", args![lane_id]);
    assert_eq!(network.state(lane_id).close_requested_epoch, Some(10));
    network.signed_call_fails(&buyer, "request_close", args![lane_id], "close was already requested");
    let tx = network.refund_transaction(&buyer, lane_id);
    network.rejected(tx, vec![buyer.proof.clone()], "close window has not passed");
    let tx = network.funded_call(&buyer, 100, "fund_lane", args![lane_id]);
    network.rejected(tx, vec![buyer.proof.clone()], "lane is closing or closed");

    // The provider can still claim its latest voucher inside the window.
    network.set_epoch(15);
    network.claim(&provider.party, &buyer, lane_id, 1_800);
    let tx = network.refund_transaction(&buyer, lane_id);
    network.rejected(tx, vec![buyer.proof.clone()], "close window has not passed");

    // Once the window has passed only the buyer can refund, and only once.
    network.set_epoch(16);
    let tx = network.refund_transaction(&stranger, lane_id);
    network.rejected(tx, vec![stranger.proof.clone()], "only the lane's buyer can refund it");
    assert_eq!(network.refund(&buyer, lane_id), Amount::from(3_200u64));
    let state = network.state(lane_id);
    assert!(state.closed);
    assert_eq!((state.refunded, state.remaining()), (Amount::from(3_200u64), Amount::zero()));
    let tx = network.refund_transaction(&buyer, lane_id);
    network.rejected(tx, vec![buyer.proof.clone()], "nothing to refund");
    network.assert_conservation(&[lane_id]);

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
    let lane_id = network.open_lane(&buyer, &provider, 5_000);
    let tx = network.funded_call(&buyer, 1_000, "fund_lane", args![lane_id]);
    network.test.execute_expect_success(tx, vec![buyer.proof.clone()]);
    let tx = network.funded_call(&stranger, 1, "fund_lane", args![lane_id]);
    network.rejected(tx, vec![stranger.proof.clone()], "only the lane's buyer can fund it");
    assert_eq!(network.state(lane_id).deposited, Amount::from(6_000u64));
    network.claim(&provider.party, &buyer, lane_id, 400);

    network.signed_call_fails(&buyer, "release_lane", args![lane_id], "only the lane's provider can release it");
    network.signed_call(&provider.party, "release_lane", args![lane_id]);
    network.signed_call_fails(&provider.party, "release_lane", args![lane_id], "lane is closed");

    // A released lane refunds at once, with no close window.
    assert_eq!(network.refund(&buyer, lane_id), Amount::from(5_600u64));
    network.assert_conservation(&[lane_id]);
}

#[test]
fn lanes_roll_over_into_a_new_page_vault() {
    let mut network = Network::deploy();
    let provider = network.provider("alice");
    let buyer = funded(&mut network.test);
    // A transaction may emit at most 256 events (withdrawals emit some too), so open lanes in batches.
    let ids: Vec<u64> = (1..=PAGE_SIZE + 1).collect();
    for batch in ids.chunks(20) {
        let mut builder = network.test.transaction();
        for _ in batch {
            builder = builder
                .call_method(buyer.account, "withdraw", args![TARI_TOKEN, 10u64])
                .put_last_instruction_output_on_workspace("funds")
                .call_method(network.ledger, "open_lane", args!["alice", buyer.public_key(), Workspace("funds")]);
        }
        let tx = builder.finish().seal(&buyer.key);
        network.test.execute_expect_success(tx, vec![buyer.proof.clone()]);
    }
    assert_eq!(network.config().4, PAGE_SIZE + 1);
    let last = PAGE_SIZE + 1;
    assert_eq!(network.lane(last).0.id, last);
    network.claim(&provider.party, &buyer, last, 10);
    network.claim(&provider.party, &buyer, 1, 5);
    network.assert_conservation(&ids);
}
