//! LaneLedger: buyer funds and per-provider payment lanes, with no admin.
//!
//! Providers are agents in the Ootle Agent Directory (github.com/alex3000-ai/ootle-agent-directory).
//! Buyers deposit the payment resource and open a lane to a provider by its directory name; the lane
//! records the card's agent key at that moment, and only that key can claim from or release the lane.
//! As work is done, the buyer signs lane vouchers carrying the cumulative amount spent on that lane;
//! the provider claims the latest voucher whenever it likes. Vouchers are signed with a voucher key the
//! buyer names when opening the lane, so the wallet key never has to sign off-chain messages and a
//! leaked voucher key can spend at most that one lane's escrow. A lane only ever holds funds escrowed
//! for that one provider, so a buyer cannot spend the same balance twice.
//!
//! Every setting is fixed when the ledger is created: the directory, the payment resource and the close
//! window. There is no fee, no pause and no owner. Providers publish a listing (e.g. a service page URL)
//! against their directory name.
//!
//! Funds sit in one vault. Accounting invariant:
//! `vault balance == sum(balances) + sum(open lane deposited - claimed)`.

use std::collections::BTreeMap;

use minicbor::{CborLen, Decode, Encode};
use tari_template_lib::prelude::*;

/// Signature domain for lane vouchers. Buyers sign with the same domain off-chain.
pub const LANE_VOUCHER_DOMAIN: &[u8] = b"ootle-agent-lane-voucher-v1";

/// Longest listing a provider can publish.
pub const LISTING_MAX: usize = 300;

/// The exact bytes a buyer signs for a lane voucher. Binding the ledger address stops a voucher
/// from being replayed against another deployment.
pub fn lane_voucher_message(ledger: &ComponentAddress, lane_id: u64, spent_cum: Amount) -> Vec<u8> {
    format!("{ledger}|{lane_id}|{}", spent_cum.to_u128()).into_bytes()
}

/// The fields of `agent_directory::Card` this template reads, in the same CBOR layout.
#[derive(Debug, Clone, Encode, Decode, CborLen)]
pub struct DirectoryCard {
    #[n(0)]
    pub agent_key: RistrettoPublicKeyBytes,
    #[n(1)]
    pub owner_key: RistrettoPublicKeyBytes,
    #[n(2)]
    pub messaging_address: String,
    #[n(3)]
    pub description: String,
    #[n(4)]
    pub channel: Option<ComponentAddress>,
    #[n(5)]
    pub status: u8,
    #[n(6)]
    pub nonce: u64,
    #[n(7)]
    pub updated_epoch: u64,
}

#[derive(Debug, Clone, Encode, Decode, CborLen)]
pub struct Lane {
    #[n(0)]
    pub buyer: RistrettoPublicKeyBytes,
    /// The provider's directory name.
    #[n(1)]
    pub provider: String,
    /// The provider card's agent key when the lane opened: the only key that can claim or release it.
    #[n(2)]
    pub provider_key: RistrettoPublicKeyBytes,
    /// Total escrowed into the lane so far.
    #[n(3)]
    pub deposited: Amount,
    /// Highest cumulative voucher amount claimed so far.
    #[n(4)]
    pub claimed: Amount,
    #[n(5)]
    pub close_requested_epoch: Option<u64>,
    #[n(6)]
    pub closed: bool,
    /// The only key whose vouchers this lane accepts.
    #[n(7)]
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
        directory: ComponentAddress,
        lane_close_epochs: u64,
        vault: Vault,
        balances: BTreeMap<RistrettoPublicKeyBytes, Amount>,
        lanes: BTreeMap<u64, Lane>,
        next_lane_id: u64,
        /// Directory name → the provider's listing.
        listings: BTreeMap<String, String>,
    }

    impl LaneLedger {
        /// Creates a ledger over `directory`'s agents. Nothing can change these settings later.
        pub fn new(directory: ComponentAddress, payment_resource: ResourceAddress, lane_close_epochs: u64) -> Component<Self> {
            assert!(lane_close_epochs > 0, "the close window must be at least one epoch");
            Component::new(Self {
                directory,
                lane_close_epochs,
                vault: Vault::new_empty(payment_resource),
                balances: BTreeMap::new(),
                lanes: BTreeMap::new(),
                next_lane_id: 1,
                listings: BTreeMap::new(),
            })
            .with_access_rules(AccessRules::new().default(rule!(allow_all)))
            .with_owner_rule(OwnerRule::None)
            .create()
        }

        // ---- providers ----

        /// Publishes the listing for directory agent `name` (empty removes it). Only the card's
        /// current agent key can do this, and only while the agent is active.
        pub fn set_listing(&mut self, name: String, listing: String) {
            let card = self.card(&name);
            assert!(CallerContext::transaction_signer_public_key() == card.agent_key, "only the agent's current key can do this");
            assert!(listing.len() <= LISTING_MAX, "listing is too long");
            if listing.is_empty() {
                self.listings.remove(&name);
            } else {
                self.listings.insert(name.clone(), listing.clone());
            }
            emit_event("lanes.listing_set", metadata!("name" => name, "listing" => listing));
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

        /// Escrows `amount` from the signer's balance into a new lane to the directory agent named
        /// `provider`, which must be active. `voucher_key` signs the lane's vouchers; it may be the
        /// buyer's own key.
        pub fn open_lane(&mut self, provider: String, amount: Amount, voucher_key: RistrettoPublicKeyBytes) -> u64 {
            assert!(amount.is_positive(), "lane amount must be positive");
            let provider_key = self.card(&provider).agent_key;
            let buyer = CallerContext::transaction_signer_public_key();
            assert!(buyer != provider_key, "cannot open a lane to yourself");

            self.debit(buyer, amount);
            let lane_id = self.next_lane_id;
            self.next_lane_id += 1;
            emit_event(
                "lanes.lane_opened",
                metadata!(
                    "lane_id" => lane_id.to_string(),
                    "buyer" => buyer.to_string(),
                    "provider" => provider.clone(),
                    "provider_key" => provider_key.to_string(),
                    "amount" => amount.to_string(),
                    "voucher_key" => voucher_key.to_string(),
                ),
            );
            self.lanes.insert(lane_id, Lane {
                buyer,
                provider,
                provider_key,
                deposited: amount,
                claimed: Amount::zero(),
                close_requested_epoch: None,
                closed: false,
                voucher_key,
            });
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

        /// Pays the signer (the lane's provider key) for a voucher signed by the lane's voucher key.
        /// `spent_cum` is the cumulative amount the buyer has authorised on this lane; the provider
        /// receives the part not yet claimed.
        pub fn claim(&mut self, lane_id: u64, spent_cum: Amount, buyer_signature: SignaturePayload) -> Bucket {
            let ledger = CallerContext::current_component_address();
            let lane = self.lanes.get(&lane_id).expect("unknown lane").clone();
            assert!(lane.provider_key == CallerContext::transaction_signer_public_key(), "only the lane's provider can claim");
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

            let payout = spent_cum.checked_sub(lane.claimed).expect("checked above");
            self.lanes.get_mut(&lane_id).expect("unknown lane").claimed = spent_cum;
            emit_event(
                "lanes.lane_claimed",
                metadata!("lane_id" => lane_id.to_string(), "spent_cum" => spent_cum.to_string(), "payout" => payout.to_string()),
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
            let lane = self.lanes.get(&lane_id).expect("unknown lane").clone();
            assert!(!lane.closed, "lane is closed");
            let requested = lane.close_requested_epoch.expect("close was not requested");
            assert!(
                Consensus::current_epoch() >= requested.saturating_add(self.lane_close_epochs),
                "close window has not passed"
            );
            self.settle_lane(lane_id, lane);
        }

        /// The provider closes a lane immediately, returning the unclaimed remainder to the buyer.
        pub fn release_lane(&mut self, lane_id: u64) {
            let lane = self.lanes.get(&lane_id).expect("unknown lane").clone();
            assert!(lane.provider_key == CallerContext::transaction_signer_public_key(), "only the lane's provider can release it");
            assert!(!lane.closed, "lane is closed");
            self.settle_lane(lane_id, lane);
        }

        // ---- reads ----

        /// (directory, payment resource, close window in epochs).
        pub fn config(&self) -> (ComponentAddress, ResourceAddress, u64) {
            (self.directory, self.vault.resource_address(), self.lane_close_epochs)
        }

        pub fn listing(&self, name: String) -> Option<String> {
            self.listings.get(&name).cloned()
        }

        pub fn balance(&self, buyer: RistrettoPublicKeyBytes) -> Amount {
            self.balances.get(&buyer).copied().unwrap_or_default()
        }

        pub fn lane(&self, lane_id: u64) -> Option<Lane> {
            self.lanes.get(&lane_id).cloned()
        }

        pub fn vault_balance(&self) -> Amount {
            self.vault.balance()
        }

        /// Directory agent `name`'s card; it must be active.
        fn card(&self, name: &str) -> DirectoryCard {
            let card: DirectoryCard = ComponentManager::get(self.directory).call("card", args![name.to_string()]);
            assert!(card.status == 0, "that agent is retired");
            card
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
