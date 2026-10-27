//! End-to-end tests for LanesGovernance, AgentRegistry and LaneLedger, executed in-process by
//! the Ootle engine (`tari_template_test_tooling`).

use lanes_ledger::{lane_voucher_message, LANE_VOUCHER_DOMAIN};
use lanes_registry::{POLICY_ANYONE, POLICY_HUMAN_ROOT, POLICY_HUMAN_VERIFIED};
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
    Amount, ComponentAddress, Hash32, NonFungibleAddress, ResourceAddress,
};
use tari_template_test_tooling::TemplateTest;

const FEE_BPS: u16 = 1_500;
const LANE_CLOSE_EPOCHS: u64 = 6;

struct Party {
    account: ComponentAddress,
    proof: NonFungibleAddress,
    key: RistrettoSecretKey,
}

impl Party {
    fn public_key(&self) -> RistrettoPublicKeyBytes {
        RistrettoPublicKey::from_secret_key(&self.key).to_byte_type()
    }
}

struct Network {
    test: TemplateTest,
    admin: Party,
    badge: ResourceAddress,
    governance: ComponentAddress,
    registry: ComponentAddress,
    ledger: ComponentAddress,
    rules_hash: Hash32,
}

fn hash(byte: u8) -> Hash32 {
    Hash32::from_array([byte; 32])
}

fn funded(test: &mut TemplateTest) -> Party {
    let (account, proof, key) = test.create_funded_account();
    Party { account, proof, key }
}

/// The component a constructor (the transaction's first instruction) returned.
fn constructed_component(test: &TemplateTest) -> ComponentAddress {
    *test.return_values()[0]
        .component_addresses()
        .first()
        .expect("constructor returned no component")
}

/// Signs a lane voucher exactly as a gateway would.
fn sign_voucher(key: &RistrettoSecretKey, ledger: &ComponentAddress, lane_id: u64, spent_cum: Amount) -> SignaturePayload {
    let message = lane_voucher_message(ledger, lane_id, spent_cum);
    let public_key = RistrettoPublicKey::from_secret_key(key);
    let (nonce, public_nonce) = RistrettoPublicKey::random_keypair(&mut rand::rng());
    let challenge = RistrettoSchnorrBlake2bVerifier::compute_challenge(
        LANE_VOUCHER_DOMAIN,
        &message,
        &public_key.to_byte_type(),
        &public_nonce.to_byte_type(),
    );
    let signature = RistrettoSchnorr::sign_raw_uniform(key, nonce, &challenge).expect("sign voucher");
    SignaturePayload::RistrettoSchnorrBlake2b(signature.to_byte_type())
}

impl Network {
    /// Deploys governance, registry and ledger and sets the rules.
    fn deploy() -> Self {
        let mut test = TemplateTest::new(env!("CARGO_MANIFEST_DIR"), ["../governance", "../registry", "../ledger"]);
        let admin = funded(&mut test);

        let governance_template = test.get_template_address("LanesGovernance");
        test.execute_expect_success(
            test.transaction()
                .call_function(governance_template, "new", args![FEE_BPS, LANE_CLOSE_EPOCHS])
                .put_last_instruction_output_on_workspace("gov")
                .call_method(admin.account, "deposit", args![Workspace("gov.1")])
                .finish()
                .seal(&admin.key),
            vec![admin.proof.clone()],
        );
        let governance = constructed_component(&test);
        let badge: ResourceAddress = test.call_method(governance, "admin_badge", args![], vec![]);

        let registry_template = test.get_template_address("AgentRegistry");
        test.execute_expect_success(
            test.transaction()
                .call_function(registry_template, "new", args![badge])
                .finish()
                .seal(&admin.key),
            vec![admin.proof.clone()],
        );
        let registry = constructed_component(&test);

        let ledger_template = test.get_template_address("LaneLedger");
        test.execute_expect_success(
            test.transaction()
                .call_function(ledger_template, "new", args![governance, registry, TARI_TOKEN])
                .finish()
                .seal(&admin.key),
            vec![admin.proof.clone()],
        );
        let ledger = constructed_component(&test);

        let mut network = Self {
            test,
            admin,
            badge,
            governance,
            registry,
            ledger,
            rules_hash: hash(0x11),
        };
        network.admin_call(governance, "set_rules", args![1u32, network.rules_hash]);
        network
    }

    /// Calls an admin-gated method with a proof of the admin badge.
    fn admin_call(&mut self, component: ComponentAddress, method: &str, args: Vec<NamedArg>) {
        let transaction = self
            .test
            .transaction()
            .call_method(self.admin.account, "create_proof_for_resource", args![self.badge])
            .put_last_instruction_output_on_workspace("badge_proof")
            .call_method(component, method, args)
            .drop_all_proofs_in_workspace()
            .finish()
            .seal(&self.admin.key);
        self.test.execute_expect_success(transaction, vec![self.admin.proof.clone()]);
    }

    /// A provider that registered itself with the `Anyone` buyer policy.
    fn provider(&mut self) -> Party {
        self.provider_with_policy(POLICY_ANYONE)
    }

    fn provider_with_policy(&mut self, buyer_policy: u8) -> Party {
        let provider = funded(&mut self.test);
        self.registry_call(&provider, "register_provider", args!["https://example.org/service", buyer_policy]);
        provider
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

    fn open_lane(&mut self, buyer: &Party, provider: &Party, amount: u64) -> u64 {
        let transaction = self
            .test
            .transaction()
            .call_method(self.ledger, "open_lane", args![provider.public_key(), Amount::from(amount), self.rules_hash, buyer.public_key()])
            .finish()
            .seal(&buyer.key);
        self.test.execute_expect_success(transaction, vec![buyer.proof.clone()]);
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

    fn registry_call(&mut self, party: &Party, method: &str, args: Vec<NamedArg>) {
        let transaction = self.test.transaction().call_method(self.registry, method, args).finish().seal(&party.key);
        self.test.execute_expect_success(transaction, vec![party.proof.clone()]);
    }

    fn registry_call_fails(&mut self, party: &Party, method: &str, args: Vec<NamedArg>, expected: &str) {
        let transaction = self.test.transaction().call_method(self.registry, method, args).finish().seal(&party.key);
        self.rejected(transaction, vec![party.proof.clone()], expected);
    }

    fn profile_tag(&mut self, party: &Party) -> Option<String> {
        self.test.call_method(self.registry, "profile_tag", args![party.public_key()], vec![])
    }

    fn human_verified_root(&mut self, party: &Party) -> bool {
        self.test.call_method(self.registry, "human_verified_root", args![party.public_key()], vec![])
    }

    /// `Profile` is CBOR-only, like `Lane`.
    fn profile(&mut self, party: &Party) -> lanes_registry::Profile {
        let transaction = self
            .test
            .transaction()
            .call_method(self.registry, "profile", args![party.public_key()])
            .finish()
            .seal(&self.admin.key);
        self.test.execute_expect_success(transaction, vec![self.admin.proof.clone()]);
        let profile: Option<lanes_registry::Profile> = self.test.return_values()[0].decoded().expect("decode profile");
        profile.expect("profile exists")
    }

    /// `Provider` is CBOR-only, like `Lane`.
    fn provider_record(&mut self, party: &Party) -> lanes_registry::Provider {
        let transaction = self
            .test
            .transaction()
            .call_method(self.registry, "provider", args![party.public_key()])
            .finish()
            .seal(&self.admin.key);
        self.test.execute_expect_success(transaction, vec![self.admin.proof.clone()]);
        let provider: Option<lanes_registry::Provider> = self.test.return_values()[0].decoded().expect("decode provider");
        provider.expect("provider exists")
    }

    fn balance(&mut self, party: &Party) -> Amount {
        self.test.call_method(self.ledger, "balance", args![party.public_key()], vec![])
    }

    /// `Lane` only implements CBOR decoding, so read it from a transaction's return value rather
    /// than through `call_method` (which also requires serde).
    fn lane(&mut self, lane_id: u64) -> lanes_ledger::Lane {
        let transaction = self.test.transaction().call_method(self.ledger, "lane", args![lane_id]).finish().seal(&self.admin.key);
        self.test.execute_expect_success(transaction, vec![self.admin.proof.clone()]);
        let lane: Option<lanes_ledger::Lane> = self.test.return_values()[0].decoded().expect("decode lane");
        lane.expect("lane exists")
    }

    fn account_balance(&mut self, party: &Party) -> Amount {
        self.test.call_method(party.account, "balance", args![TARI_TOKEN], vec![])
    }

    fn set_epoch(&mut self, epoch: u64) {
        self.test.set_virtual_substate(VirtualSubstateId::CurrentEpoch, VirtualSubstate::CurrentEpoch(epoch));
    }

    /// vault == buyer balances + unclaimed escrow in open lanes + fees owed.
    fn assert_conservation(&mut self, buyers: &[&Party], lanes: &[u64]) {
        let vault: Amount = self.test.call_method(self.ledger, "vault_balance", args![], vec![]);
        let fees: Amount = self.test.call_method(self.ledger, "fees_owed", args![], vec![]);
        let mut expected = fees;
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

// ---- governance ----

#[test]
fn governance_setters_require_the_admin_badge() {
    let mut network = Network::deploy();
    let outsider = funded(&mut network.test);
    let transaction = network
        .test
        .transaction()
        .call_method(network.governance, "set_params", args![0u16, 1u64])
        .finish()
        .seal(&outsider.key);
    network.rejected(transaction, vec![outsider.proof.clone()], "Access Denied");

    network.admin_call(network.governance, "set_params", args![1_000u16, 3u64]);
    let config: (u16, u64, Hash32, bool) = network.test.call_method(network.governance, "ledger_config", args![], vec![]);
    assert_eq!(config, (1_000, 3, network.rules_hash, false));
}

#[test]
fn governance_rejects_fees_above_the_cap() {
    let mut network = Network::deploy();
    let transaction = network
        .test
        .transaction()
        .call_method(network.admin.account, "create_proof_for_resource", args![network.badge])
        .put_last_instruction_output_on_workspace("badge_proof")
        .call_method(network.governance, "set_params", args![1_501u16, 6u64])
        .drop_all_proofs_in_workspace()
        .finish()
        .seal(&network.admin.key);
    network.rejected(transaction, vec![network.admin.proof.clone()], "fee_bps exceeds the 15% cap");
}

// ---- registry ----

#[test]
fn providers_register_themselves_and_admin_ops_need_the_badge() {
    let mut network = Network::deploy();
    let provider = network.provider();
    let active: bool = network.test.call_method(network.registry, "is_active_provider", args![provider.public_key()], vec![]);
    assert!(active);
    network.registry_call_fails(&provider, "register_provider", args!["", POLICY_ANYONE], "provider already registered");

    // A provider cannot ban anyone, itself included.
    network.registry_call_fails(&provider, "ban_provider", args![provider.public_key()], "Access Denied");

    network.admin_call(network.registry, "ban_provider", args![provider.public_key()]);
    let active: bool = network.test.call_method(network.registry, "is_active_provider", args![provider.public_key()], vec![]);
    assert!(!active);
    network.admin_call(network.registry, "unban_provider", args![provider.public_key()]);
    let active: bool = network.test.call_method(network.registry, "is_active_provider", args![provider.public_key()], vec![]);
    assert!(active);
}

#[test]
fn providers_manage_their_own_listing_and_policy() {
    let mut network = Network::deploy();
    let provider = network.provider();
    let stranger = funded(&mut network.test);

    network.registry_call(&provider, "set_listing", args!["https://ootlebook.example/services/summaries"]);
    network.registry_call(&provider, "set_buyer_policy", args![POLICY_HUMAN_ROOT]);
    let record = network.provider_record(&provider);
    assert_eq!(record.listing, "https://ootlebook.example/services/summaries");
    assert_eq!(record.buyer_policy, POLICY_HUMAN_ROOT);

    network.registry_call_fails(&provider, "set_buyer_policy", args![3u8], "unknown buyer policy");
    network.registry_call_fails(&provider, "set_listing", args!["x".repeat(257)], "listing must be at most 256 bytes");
    network.registry_call_fails(&stranger, "set_listing", args!["mine now"], "provider not registered");
}

// ---- registry: buyer profiles ----

#[test]
fn agents_register_their_own_wallet_profile_without_an_operator() {
    let mut network = Network::deploy();
    let agent = funded(&mut network.test);
    assert_eq!(network.profile_tag(&agent), None);

    network.registry_call(&agent, "register_profile", args![None::<RistrettoPublicKeyBytes>]);
    assert_eq!(network.profile_tag(&agent).as_deref(), Some(lanes_registry::TAG_OWN_WALLET));
    assert!(!network.human_verified_root(&agent));
    network.registry_call_fails(&agent, "register_profile", args![None::<RistrettoPublicKeyBytes>], "profile already registered");
}

#[test]
fn sub_agent_links_need_both_the_child_and_the_parent() {
    let mut network = Network::deploy();
    let manager = funded(&mut network.test);
    let child = funded(&mut network.test);
    let stranger = funded(&mut network.test);
    network.registry_call_fails(&child, "register_profile", args![Some(manager.public_key())], "parent has no profile");

    network.registry_call(&manager, "register_profile", args![None::<RistrettoPublicKeyBytes>]);
    network.registry_call(&stranger, "register_profile", args![None::<RistrettoPublicKeyBytes>]);
    network.registry_call(&child, "register_profile", args![Some(manager.public_key())]);
    assert_eq!(network.profile(&child).parent, None);

    network.registry_call_fails(&stranger, "confirm_sub_agent", args![child.public_key()], "child did not ask for this parent");
    network.registry_call(&manager, "confirm_sub_agent", args![child.public_key()]);
    let profile = network.profile(&child);
    assert_eq!(profile.parent, Some(manager.public_key()));
    assert_eq!(profile.pending_parent, None);

    network.registry_call_fails(&stranger, "detach_sub_agent", args![child.public_key()], "only the child or its parent can detach");
    network.registry_call(&child, "detach_sub_agent", args![child.public_key()]);
    assert_eq!(network.profile(&child).parent, None);

    // A detached sub-agent can ask for a new parent; the old one can no longer confirm it.
    network.registry_call(&child, "request_parent", args![stranger.public_key()]);
    network.registry_call_fails(&manager, "confirm_sub_agent", args![child.public_key()], "child did not ask for this parent");
    network.registry_call(&stranger, "confirm_sub_agent", args![child.public_key()]);
    assert_eq!(network.profile(&child).parent, Some(stranger.public_key()));
}

#[test]
fn human_verification_comes_from_admin_appointed_attesters_and_expires() {
    let mut network = Network::deploy();
    let attester = funded(&mut network.test);
    let human = funded(&mut network.test);
    let sub_agent = funded(&mut network.test);
    network.registry_call(&human, "register_profile", args![None::<RistrettoPublicKeyBytes>]);
    network.registry_call(&sub_agent, "register_profile", args![Some(human.public_key())]);
    network.registry_call(&human, "confirm_sub_agent", args![sub_agent.public_key()]);

    // Only the admin appoints attesters, and only attesters attest.
    network.registry_call_fails(&attester, "add_attester", args![attester.public_key()], "Access Denied");
    network.registry_call_fails(&attester, "attest_human", args![human.public_key(), hash(0x5e), 20u64], "signer is not an attester");
    network.admin_call(network.registry, "add_attester", args![attester.public_key()]);

    network.set_epoch(10);
    network.registry_call_fails(&attester, "attest_human", args![human.public_key(), hash(0x5e), 10u64], "attestation must expire in the future");
    network.registry_call(&attester, "attest_human", args![human.public_key(), hash(0x5e), 20u64]);
    assert_eq!(network.profile_tag(&human).as_deref(), Some(lanes_registry::TAG_HUMAN_VERIFIED));
    // The sub-agent keeps its own tag but answers to a verified human.
    assert_eq!(network.profile_tag(&sub_agent).as_deref(), Some(lanes_registry::TAG_OWN_WALLET));
    assert!(network.human_verified_root(&sub_agent));

    network.set_epoch(20);
    assert_eq!(network.profile_tag(&human).as_deref(), Some(lanes_registry::TAG_OWN_WALLET));
    assert!(!network.human_verified_root(&sub_agent));

    // Re-attested, then revoked by the subject itself.
    network.registry_call(&attester, "attest_human", args![human.public_key(), hash(0x5f), 40u64]);
    network.registry_call_fails(&sub_agent, "revoke_human", args![human.public_key()], "only an attester or the subject can revoke");
    network.registry_call(&human, "revoke_human", args![human.public_key()]);
    assert_eq!(network.profile_tag(&human).as_deref(), Some(lanes_registry::TAG_OWN_WALLET));
}

// ---- ledger: lanes ----

#[test]
fn lane_claims_pay_the_provider_less_the_fee() {
    let mut network = Network::deploy();
    let provider = network.provider();
    let buyer = funded(&mut network.test);

    network.deposit(&buyer, 10_000);
    let lane_id = network.open_lane(&buyer, &provider, 6_000);
    assert_eq!(network.balance(&buyer), Amount::from(4_000u64));
    network.assert_conservation(&[&buyer], &[lane_id]);

    let before = network.account_balance(&provider);
    network.claim(&provider, &buyer, lane_id, 1_000);
    // 15% fee: provider gets 850 of 1000.
    assert_eq!(network.account_balance(&provider).checked_sub(before).unwrap(), Amount::from(850u64));
    network.assert_conservation(&[&buyer], &[lane_id]);

    // A later voucher pays only the new part: 2500 - 1000 = 1500, provider gets 1275.
    network.claim(&provider, &buyer, lane_id, 2_500);
    assert_eq!(network.account_balance(&provider).checked_sub(before).unwrap(), Amount::from(2_125u64));
    let fees: Amount = network.test.call_method(network.ledger, "fees_owed", args![], vec![]);
    assert_eq!(fees, Amount::from(375u64));
    assert_eq!(network.lane(lane_id).claimed, Amount::from(2_500u64));
    network.assert_conservation(&[&buyer], &[lane_id]);
}

#[test]
fn claims_reject_bad_vouchers() {
    let mut network = Network::deploy();
    let provider = network.provider();
    let buyer = funded(&mut network.test);
    let stranger = funded(&mut network.test);
    network.deposit(&buyer, 10_000);
    let lane_id = network.open_lane(&buyer, &provider, 5_000);

    // Signed by someone other than the lane's voucher key.
    let forged = sign_voucher(&stranger.key, &network.ledger, lane_id, Amount::from(1_000u64));
    let transaction = network.claim_transaction(&provider, lane_id, Amount::from(1_000u64), forged);
    network.rejected(transaction, vec![provider.proof.clone()], "invalid voucher signature");

    // A genuine signature over a different amount.
    let other_amount = sign_voucher(&buyer.key, &network.ledger, lane_id, Amount::from(900u64));
    let transaction = network.claim_transaction(&provider, lane_id, Amount::from(1_000u64), other_amount);
    network.rejected(transaction, vec![provider.proof.clone()], "invalid voucher signature");

    // More than the lane holds, even though the buyer signed it.
    let too_much = sign_voucher(&buyer.key, &network.ledger, lane_id, Amount::from(5_001u64));
    let transaction = network.claim_transaction(&provider, lane_id, Amount::from(5_001u64), too_much);
    network.rejected(transaction, vec![provider.proof.clone()], "voucher exceeds the lane's escrow");

    // Only the lane's provider may claim.
    let valid = sign_voucher(&buyer.key, &network.ledger, lane_id, Amount::from(1_000u64));
    let transaction = network.claim_transaction(&stranger, lane_id, Amount::from(1_000u64), valid.clone());
    network.rejected(transaction, vec![stranger.proof.clone()], "only the lane's provider can claim");

    // The valid voucher works once; replaying it pays nothing more.
    let transaction = network.claim_transaction(&provider, lane_id, Amount::from(1_000u64), valid.clone());
    network.test.execute_expect_success(transaction, vec![provider.proof.clone()]);
    let transaction = network.claim_transaction(&provider, lane_id, Amount::from(1_000u64), valid);
    network.rejected(transaction, vec![provider.proof.clone()], "voucher does not exceed the amount already claimed");
    network.assert_conservation(&[&buyer], &[lane_id]);
}

#[test]
fn only_the_lanes_voucher_key_can_sign_vouchers() {
    let mut network = Network::deploy();
    let provider = network.provider();
    let buyer = funded(&mut network.test);
    network.deposit(&buyer, 10_000);

    // The buyer keeps its wallet key in a wallet daemon and signs vouchers with a separate hot key.
    let voucher_secret = RistrettoSecretKey::random(&mut rand::rng());
    let voucher_key: RistrettoPublicKeyBytes = RistrettoPublicKey::from_secret_key(&voucher_secret).to_byte_type();
    network.signed_call(&buyer, "open_lane", args![provider.public_key(), Amount::from(5_000u64), network.rules_hash, voucher_key]);
    let lane_id = network.test.return_values()[0].decoded::<u64>().expect("lane id");
    assert_eq!(network.lane(lane_id).voucher_key, voucher_key);

    let spent_cum = Amount::from(1_000u64);
    let by_wallet_key = sign_voucher(&buyer.key, &network.ledger, lane_id, spent_cum);
    let transaction = network.claim_transaction(&provider, lane_id, spent_cum, by_wallet_key);
    network.rejected(transaction, vec![provider.proof.clone()], "invalid voucher signature");

    let by_voucher_key = sign_voucher(&voucher_secret, &network.ledger, lane_id, spent_cum);
    let transaction = network.claim_transaction(&provider, lane_id, spent_cum, by_voucher_key);
    network.test.execute_expect_success(transaction, vec![provider.proof.clone()]);
    network.assert_conservation(&[&buyer], &[lane_id]);
}

#[test]
fn lanes_need_rules_consent_and_an_active_provider() {
    let mut network = Network::deploy();
    let provider = network.provider();
    let unregistered = funded(&mut network.test);
    let buyer = funded(&mut network.test);
    network.deposit(&buyer, 10_000);

    network.signed_call_fails(&buyer, "open_lane", args![provider.public_key(), Amount::from(100u64), hash(0x99), buyer.public_key()], "rules consent does not match");
    network.signed_call_fails(&buyer, "open_lane", args![unregistered.public_key(), Amount::from(100u64), network.rules_hash, buyer.public_key()], "provider is not an active registered provider");
    network.signed_call_fails(&buyer, "open_lane", args![provider.public_key(), Amount::from(10_001u64), network.rules_hash, buyer.public_key()], "insufficient balance");

    network.admin_call(network.registry, "ban_provider", args![provider.public_key()]);
    network.signed_call_fails(&buyer, "open_lane", args![provider.public_key(), Amount::from(100u64), network.rules_hash, buyer.public_key()], "provider is not an active registered provider");
}

#[test]
fn providers_can_require_human_backed_buyers() {
    let mut network = Network::deploy();
    let root_only = network.provider_with_policy(POLICY_HUMAN_ROOT);
    let verified_only = network.provider_with_policy(POLICY_HUMAN_VERIFIED);
    let attester = funded(&mut network.test);
    let human = funded(&mut network.test);
    let sub_agent = funded(&mut network.test);
    let lone_agent = funded(&mut network.test);
    network.admin_call(network.registry, "add_attester", args![attester.public_key()]);
    network.registry_call(&human, "register_profile", args![None::<RistrettoPublicKeyBytes>]);
    network.registry_call(&sub_agent, "register_profile", args![Some(human.public_key())]);
    network.registry_call(&human, "confirm_sub_agent", args![sub_agent.public_key()]);
    network.registry_call(&attester, "attest_human", args![human.public_key(), hash(0x5e), 100u64]);
    for buyer in [&human, &sub_agent, &lone_agent] {
        network.deposit(buyer, 1_000);
    }
    let policy_error = "buyer does not meet the provider's buyer policy";
    let amount = Amount::from(100u64);

    // An agent with no profile, or no human behind it, is turned away by both.
    network.signed_call_fails(&lone_agent, "open_lane", args![root_only.public_key(), amount, network.rules_hash, lone_agent.public_key()], policy_error);
    network.signed_call_fails(&lone_agent, "open_lane", args![verified_only.public_key(), amount, network.rules_hash, lone_agent.public_key()], policy_error);

    // A sub-agent of a verified human passes `HumanRoot` but not `HumanVerified`.
    network.open_lane(&sub_agent, &root_only, 100);
    network.signed_call_fails(&sub_agent, "open_lane", args![verified_only.public_key(), amount, network.rules_hash, sub_agent.public_key()], policy_error);

    // The verified human passes both.
    network.open_lane(&human, &root_only, 100);
    network.open_lane(&human, &verified_only, 100);

    // Dropping the policy opens the provider to everyone.
    network.registry_call(&verified_only, "set_buyer_policy", args![POLICY_ANYONE]);
    network.open_lane(&lone_agent, &verified_only, 100);
}

#[test]
fn closing_a_lane_waits_for_the_window_and_returns_the_remainder() {
    let mut network = Network::deploy();
    let provider = network.provider();
    let buyer = funded(&mut network.test);
    network.deposit(&buyer, 10_000);
    let lane_id = network.open_lane(&buyer, &provider, 5_000);
    network.claim(&provider, &buyer, lane_id, 1_000);

    network.set_epoch(10);
    network.signed_call(&buyer, "request_close", args![lane_id]);
    network.signed_call_fails(&buyer, "finalize_close", args![lane_id], "close window has not passed");

    // The provider can still claim its latest voucher inside the window.
    network.set_epoch(15);
    network.claim(&provider, &buyer, lane_id, 1_800);
    network.signed_call_fails(&buyer, "finalize_close", args![lane_id], "close window has not passed");

    network.set_epoch(16);
    network.signed_call(&buyer, "finalize_close", args![lane_id]);
    assert!(network.lane(lane_id).closed);
    assert_eq!(network.balance(&buyer), Amount::from(5_000u64 + 3_200));
    network.assert_conservation(&[&buyer], &[lane_id]);

    // Nothing more can be claimed from a closed lane.
    let late = sign_voucher(&buyer.key, &network.ledger, lane_id, Amount::from(2_000u64));
    let transaction = network.claim_transaction(&provider, lane_id, Amount::from(2_000u64), late);
    network.rejected(transaction, vec![provider.proof.clone()], "lane is closed");
}

#[test]
fn providers_can_release_a_lane_early() {
    let mut network = Network::deploy();
    let provider = network.provider();
    let buyer = funded(&mut network.test);
    network.deposit(&buyer, 10_000);
    let lane_id = network.open_lane(&buyer, &provider, 5_000);
    network.claim(&provider, &buyer, lane_id, 400);

    network.signed_call_fails(&buyer, "release_lane", args![lane_id], "only the lane's provider can release it");
    network.signed_call(&provider, "release_lane", args![lane_id]);
    assert_eq!(network.balance(&buyer), Amount::from(5_000u64 + 4_600));
    network.assert_conservation(&[&buyer], &[lane_id]);
}

#[test]
fn buyers_withdraw_unallocated_funds_and_admin_withdraws_fees() {
    let mut network = Network::deploy();
    let provider = network.provider();
    let buyer = funded(&mut network.test);
    network.deposit(&buyer, 10_000);
    let lane_id = network.open_lane(&buyer, &provider, 5_000);
    network.claim(&provider, &buyer, lane_id, 2_000);

    network.signed_call_fails(&buyer, "withdraw", args![Amount::from(5_001u64)], "insufficient balance");
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

    // Fees: only the badge holder.
    network.signed_call_fails(&buyer, "withdraw_fees", args![Amount::from(300u64)], "Access Denied");
    let transaction = network
        .test
        .transaction()
        .call_method(network.admin.account, "create_proof_for_resource", args![network.badge])
        .put_last_instruction_output_on_workspace("badge_proof")
        .call_method(network.ledger, "withdraw_fees", args![Amount::from(300u64)])
        .put_last_instruction_output_on_workspace("fees")
        .call_method(network.admin.account, "deposit", args![Workspace("fees")])
        .drop_all_proofs_in_workspace()
        .finish()
        .seal(&network.admin.key);
    network.test.execute_expect_success(transaction, vec![network.admin.proof.clone()]);
    network.assert_conservation(&[&buyer], &[lane_id]);
}
