//! LaneLedger: buyer funds and per-provider payment lanes.
//!
//! Buyers deposit the payment resource and open a lane to each provider they use. As work is done,
//! the buyer signs lane vouchers carrying the cumulative amount spent on that lane; the provider
//! claims the latest voucher whenever it likes. Vouchers are signed with a voucher key the buyer
//! names when opening the lane, so the wallet key never has to sign off-chain messages and a leaked
//! voucher key can spend at most that one lane's escrow. A lane only ever holds funds escrowed for that one
//! provider, so a buyer cannot spend the same balance twice.
//!
//! Funds sit in one vault. Accounting invariant:
//! `vault balance == sum(balances) + sum(open lane deposited - claimed) + fees_owed`.

use std::collections::BTreeMap;

use minicbor::{CborLen, Decode, Encode};
use tari_template_lib::prelude::*;

/// Signature domain for lane vouchers. Buyers sign with the same domain off-chain.
pub const LANE_VOUCHER_DOMAIN: &[u8] = b"ootle-agent-lane-voucher-v1";

const BPS: u128 = 10_000;

/// The exact bytes a buyer signs for a lane voucher. Binding the ledger address stops a voucher
/// from being replayed against another deployment.
pub fn lane_voucher_message(ledger: &ComponentAddress, lane_id: u64, spent_cum: Amount) -> Vec<u8> {
    format!("{ledger}|{lane_id}|{}", spent_cum.to_u128()).into_bytes()
}

#[derive(Debug, Clone, Encode, Decode, CborLen)]
pub struct Lane {
    #[n(0)]
    pub buyer: RistrettoPublicKeyBytes,
    #[n(1)]
    pub provider: RistrettoPublicKeyBytes,
    /// Total escrowed into the lane so far.
    #[n(2)]
    pub deposited: Amount,
    /// Highest cumulative voucher amount claimed so far.
    #[n(3)]
    pub claimed: Amount,
    #[n(4)]
    pub close_requested_epoch: Option<u64>,
    #[n(5)]
    pub closed: bool,
    /// The only key whose vouchers this lane accepts.
    #[n(6)]
    pub voucher_key: RistrettoPublicKeyBytes,
}

impl Lane {
    fn remaining(&self) -> Amount {
        self.deposited.checked_sub(self.claimed).expect("lane claimed more than deposited")
    }
}

#[template]
mod lane_ledger {
    use super::*;

    pub struct LaneLedger {
        governance: ComponentAddress,
        registry: ComponentAddress,
        vault: Vault,
        balances: BTreeMap<RistrettoPublicKeyBytes, Amount>,
        lanes: BTreeMap<u64, Lane>,
        next_lane_id: u64,
        fees_owed: Amount,
    }

    impl LaneLedger {
        pub fn new(governance: ComponentAddress, registry: ComponentAddress, payment_resource: ResourceAddress) -> Component<Self> {
            let admin_badge: ResourceAddress = ComponentManager::get(governance).call("admin_badge", args![]);
            let access_rules = AccessRules::new()
                .default(rule!(allow_all))
                .method("withdraw_fees", rule!(resource(admin_badge)));
            Component::new(Self {
                governance,
                registry,
                vault: Vault::new_empty(payment_resource),
                balances: BTreeMap::new(),
                lanes: BTreeMap::new(),
                next_lane_id: 1,
                fees_owed: Amount::zero(),
            })
            .with_access_rules(access_rules)
            .with_owner_rule(OwnerRule::None)
            .create()
        }

        // ---- buyer funds ----

        /// Credits the transaction signer with the bucket's amount.
        pub fn deposit(&mut self, bucket: Bucket) {
            assert_eq!(bucket.resource_address(), self.vault.resource_address(), "wrong payment resource");
            let amount = bucket.amount();
            assert!(amount.is_positive(), "deposit must be positive");
            let buyer = CallerContext::transaction_signer_public_key();
            self.vault.deposit(bucket);
            self.credit(buyer, amount);
            emit_event("lanes.deposit", metadata!("buyer" => buyer.to_string(), "amount" => amount.to_string()));
        }

        /// Withdraws unallocated funds back to the signer.
        pub fn withdraw(&mut self, amount: Amount) -> Bucket {
            assert!(amount.is_positive(), "withdrawal must be positive");
            let buyer = CallerContext::transaction_signer_public_key();
            self.debit(buyer, amount);
            self.vault.withdraw(amount)
        }

        // ---- lanes ----

        /// Escrows `amount` from the signer's balance into a new lane to `provider`. `rules_hash`
        /// is the buyer's consent to the current network rules and must match governance.
        /// `voucher_key` signs the lane's vouchers; it may be the buyer's own key.
        pub fn open_lane(
            &mut self,
            provider: RistrettoPublicKeyBytes,
            amount: Amount,
            rules_hash: Hash32,
            voucher_key: RistrettoPublicKeyBytes,
        ) -> u64 {
            let (_, _, current_rules, paused) = self.ledger_config();
            assert!(!paused, "network is paused");
            assert!(rules_hash == current_rules, "rules consent does not match the current rules");
            assert!(amount.is_positive(), "lane amount must be positive");
            let buyer = CallerContext::transaction_signer_public_key();
            assert!(buyer != provider, "cannot open a lane to yourself");
            let registry = ComponentManager::get(self.registry);
            let active: bool = registry.call("is_active_provider", args![provider]);
            assert!(active, "provider is not an active registered provider");
            let allowed: bool = registry.call("buyer_allowed", args![provider, buyer]);
            assert!(allowed, "buyer does not meet the provider's buyer policy");

            self.debit(buyer, amount);
            let lane_id = self.next_lane_id;
            self.next_lane_id += 1;
            self.lanes.insert(lane_id, Lane {
                buyer,
                provider,
                deposited: amount,
                claimed: Amount::zero(),
                close_requested_epoch: None,
                closed: false,
                voucher_key,
            });
            emit_event(
                "lanes.lane_opened",
                metadata!(
                    "lane_id" => lane_id.to_string(),
                    "buyer" => buyer.to_string(),
                    "provider" => provider.to_string(),
                    "amount" => amount.to_string(),
                    "voucher_key" => voucher_key.to_string(),
                ),
            );
            lane_id
        }

        /// Adds more escrow to one of the signer's open lanes.
        pub fn fund_lane(&mut self, lane_id: u64, amount: Amount) {
            assert!(amount.is_positive(), "top-up must be positive");
            let buyer = CallerContext::transaction_signer_public_key();
            let lane = self.lanes.get(&lane_id).expect("unknown lane");
            assert!(lane.buyer == buyer, "only the lane's buyer can fund it");
            assert!(!lane.closed && lane.close_requested_epoch.is_none(), "lane is closing or closed");
            self.debit(buyer, amount);
            let lane = self.lanes.get_mut(&lane_id).expect("unknown lane");
            lane.deposited = lane.deposited.checked_add(amount).expect("lane deposit overflow");
        }

        /// Pays the signer (the lane's provider) for a voucher signed by the lane's voucher key. `spent_cum` is the
        /// cumulative amount the buyer has authorised on this lane; the provider receives the part
        /// not yet claimed, less the operator fee.
        pub fn claim(&mut self, lane_id: u64, spent_cum: Amount, buyer_signature: SignaturePayload) -> Bucket {
            let ledger = CallerContext::current_component_address();
            let (fee_bps, _, _, _) = self.ledger_config();
            let provider = CallerContext::transaction_signer_public_key();
            let lane = self.lanes.get(&lane_id).expect("unknown lane").clone();
            assert!(lane.provider == provider, "only the lane's provider can claim");
            assert!(!lane.closed, "lane is closed");
            assert!(spent_cum > lane.claimed, "voucher does not exceed the amount already claimed");
            assert!(spent_cum <= lane.deposited, "voucher exceeds the lane's escrow");

            let message = lane_voucher_message(&ledger, lane_id, spent_cum);
            let verified = SignatureVerifier::with_domain(LANE_VOUCHER_DOMAIN).verify(
                &PublicKey::Ristretto25519(lane.voucher_key),
                &message,
                &buyer_signature,
            );
            assert!(verified, "invalid voucher signature");

            let delta = spent_cum.checked_sub(lane.claimed).expect("checked above");
            let fee = Amount::new(delta.to_u128() * u128::from(fee_bps) / BPS);
            let payout = delta.checked_sub(fee).expect("fee is at most the claimed delta");
            self.fees_owed = self.fees_owed.checked_add(fee).expect("fee overflow");
            self.lanes.get_mut(&lane_id).expect("unknown lane").claimed = spent_cum;
            emit_event(
                "lanes.lane_claimed",
                metadata!(
                    "lane_id" => lane_id.to_string(),
                    "spent_cum" => spent_cum.to_string(),
                    "payout" => payout.to_string(),
                    "fee" => fee.to_string(),
                ),
            );
            self.vault.withdraw(payout)
        }

        /// The buyer starts closing a lane. The provider can still claim until it is finalized.
        pub fn request_close(&mut self, lane_id: u64) {
            let buyer = CallerContext::transaction_signer_public_key();
            let lane = self.lanes.get_mut(&lane_id).expect("unknown lane");
            assert!(lane.buyer == buyer, "only the lane's buyer can close it");
            assert!(!lane.closed, "lane is closed");
            if lane.close_requested_epoch.is_none() {
                lane.close_requested_epoch = Some(Consensus::current_epoch());
            }
        }

        /// Anyone can finalize a close once the close window has passed; the unclaimed remainder
        /// returns to the buyer's balance.
        pub fn finalize_close(&mut self, lane_id: u64) {
            let (_, lane_close_epochs, _, _) = self.ledger_config();
            let lane = self.lanes.get(&lane_id).expect("unknown lane").clone();
            assert!(!lane.closed, "lane is closed");
            let requested = lane.close_requested_epoch.expect("close was not requested");
            assert!(
                Consensus::current_epoch() >= requested.saturating_add(lane_close_epochs),
                "close window has not passed"
            );
            self.settle_lane(lane_id, lane);
        }

        /// The provider closes a lane immediately, returning the unclaimed remainder to the buyer.
        pub fn release_lane(&mut self, lane_id: u64) {
            let provider = CallerContext::transaction_signer_public_key();
            let lane = self.lanes.get(&lane_id).expect("unknown lane").clone();
            assert!(lane.provider == provider, "only the lane's provider can release it");
            assert!(!lane.closed, "lane is closed");
            self.settle_lane(lane_id, lane);
        }

        // ---- operator ----

        pub fn withdraw_fees(&mut self, amount: Amount) -> Bucket {
            self.fees_owed = self.fees_owed.checked_sub(amount).expect("amount exceeds fees owed");
            self.vault.withdraw(amount)
        }

        // ---- reads ----

        pub fn balance(&self, buyer: RistrettoPublicKeyBytes) -> Amount {
            self.balances.get(&buyer).copied().unwrap_or_default()
        }

        pub fn lane(&self, lane_id: u64) -> Option<Lane> {
            self.lanes.get(&lane_id).cloned()
        }

        pub fn fees_owed(&self) -> Amount {
            self.fees_owed
        }

        pub fn vault_balance(&self) -> Amount {
            self.vault.balance()
        }

        fn ledger_config(&self) -> (u16, u64, Hash32, bool) {
            ComponentManager::get(self.governance).call("ledger_config", args![])
        }

        fn settle_lane(&mut self, lane_id: u64, lane: Lane) {
            let remaining = lane.remaining();
            self.credit(lane.buyer, remaining);
            self.lanes.get_mut(&lane_id).expect("unknown lane").closed = true;
            emit_event(
                "lanes.lane_closed",
                metadata!("lane_id" => lane_id.to_string(), "returned" => remaining.to_string()),
            );
        }

        fn credit(&mut self, buyer: RistrettoPublicKeyBytes, amount: Amount) {
            let balance = self.balances.entry(buyer).or_default();
            *balance = balance.checked_add(amount).expect("balance overflow");
        }

        fn debit(&mut self, buyer: RistrettoPublicKeyBytes, amount: Amount) {
            let balance = self.balances.get_mut(&buyer).expect("no balance");
            *balance = balance.checked_sub(amount).expect("insufficient balance");
        }
    }
}
