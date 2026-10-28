//! LaneLedger v0.3: pay-per-use payment lanes between agents, with no admin.
//!
//! Providers are agents in the Ootle Agent Directory (github.com/alex3000-ai/ootle-agent-directory). A buyer
//! opens a lane to a provider by its directory name, escrowing a bucket of the payment resource; the lane
//! records the card's agent key at that moment, and only that key can claim from or release the lane. As work
//! is done, the buyer signs vouchers carrying the cumulative amount spent on the lane, and the provider claims
//! the latest voucher whenever it likes. Vouchers are signed with a voucher key the buyer names when opening
//! the lane, so the wallet key never signs off-chain messages and a leaked voucher key can spend at most that
//! one lane's escrow.
//!
//! To stop, the buyer requests a close; the provider can still claim until the close window has passed, and
//! then the buyer takes the remainder back with `refund`. A provider can also release a lane at once, which
//! lets the buyer refund straight away.
//!
//! Every setting is fixed when the ledger is created: the directory, the payment resource and the close
//! window. There is no fee, no pause and no owner. Lanes are non-fungible tokens of the ledger's own resource
//! with sequential ids 1, 2, 3… (`nft_<lanes resource>_u64_<id>`), kept in fixed-size page vaults, so a call
//! costs the same however many lanes the ledger has seen. Escrow sits in one vault:
//! `escrow == sum over lanes of (deposited - claimed - refunded)`.

use minicbor::{CborLen, Decode, Encode};
use tari_template_lib::prelude::*;

/// Signature domain for lane vouchers. Buyers sign with the same domain off-chain.
pub const LANE_VOUCHER_DOMAIN: &[u8] = b"ootle-agent-lane-voucher-v1";

/// Lanes per page vault.
pub const PAGE_SIZE: u64 = 256;

/// The exact bytes a buyer signs for a lane voucher. Binding the ledger address stops a voucher from being
/// replayed against another deployment.
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

/// Fixed when the lane opens.
#[derive(Debug, Clone, Encode, Decode, CborLen)]
pub struct LaneInfo {
    #[n(0)]
    pub id: u64,
    /// The key that signed the opening transaction: the only key that can fund, close or refund the lane.
    #[n(1)]
    pub buyer: RistrettoPublicKeyBytes,
    /// The provider's directory name.
    #[n(2)]
    pub provider: String,
    /// The provider card's agent key when the lane opened: the only key that can claim or release it.
    #[n(3)]
    pub provider_key: RistrettoPublicKeyBytes,
    /// The only key whose vouchers this lane accepts.
    #[n(4)]
    pub voucher_key: RistrettoPublicKeyBytes,
    #[n(5)]
    pub opened_epoch: u64,
}

#[derive(Debug, Clone, Encode, Decode, CborLen)]
pub struct LaneState {
    /// Total escrowed into the lane so far.
    #[n(0)]
    pub deposited: Amount,
    /// Highest cumulative voucher amount claimed so far.
    #[n(1)]
    pub claimed: Amount,
    /// What the buyer took back.
    #[n(2)]
    pub refunded: Amount,
    #[n(3)]
    pub close_requested_epoch: Option<u64>,
    /// Released by the provider or refunded by the buyer: no more claims.
    #[n(4)]
    pub closed: bool,
}

impl LaneState {
    pub fn remaining(&self) -> Amount {
        self.deposited.checked_sub(self.claimed).and_then(|a| a.checked_sub(self.refunded)).expect("lane paid out more than deposited")
    }
}

#[template]
mod lane_ledger {
    use super::*;

    pub struct LaneLedger {
        directory: ComponentAddress,
        lane_close_epochs: u64,
        escrow: Vault,
        lanes: ResourceAddress,
        pages: Vec<Vault>,
        count: u64,
    }

    impl LaneLedger {
        /// Creates a ledger over `directory`'s agents. Nothing can change these settings later.
        pub fn new(directory: ComponentAddress, payment_resource: ResourceAddress, lane_close_epochs: u64) -> Component<Self> {
            assert!(lane_close_epochs > 0, "the close window must be at least one epoch");
            let alloc = CallerContext::allocate_component_address(None);
            let this = rule!(component(alloc.get_address()));
            let lanes = ResourceBuilder::non_fungible()
                .with_token_symbol("LANE")
                .mintable(this.clone(), UpdateRule::Locked)
                .update_non_fungible_data(this, UpdateRule::Locked)
                .burnable(rule!(deny_all), UpdateRule::Locked)
                .recallable(rule!(deny_all), UpdateRule::Locked)
                .freezable(rule!(deny_all), UpdateRule::Locked)
                .withdrawable(rule!(deny_all), UpdateRule::Locked)
                .with_owner_rule(OwnerRule::None)
                .disable_total_supply_tracking()
                .build();
            Component::new(Self {
                directory,
                lane_close_epochs,
                escrow: Vault::new_empty(payment_resource),
                lanes,
                pages: vec![Vault::new_empty(lanes)],
                count: 0,
            })
            .with_address_allocation(alloc)
            .with_access_rules(AccessRules::new().default(rule!(allow_all)))
            .with_owner_rule(OwnerRule::None)
            .create()
        }

        /// Opens a lane from the signer to the active directory agent named `provider`, escrowing `funds`.
        /// `voucher_key` signs the lane's vouchers; it may be the buyer's own key. Returns the lane's id.
        pub fn open_lane(&mut self, provider: String, voucher_key: RistrettoPublicKeyBytes, funds: Bucket) -> u64 {
            let amount = self.take(funds);
            let card: DirectoryCard = ComponentManager::get(self.directory).call("card", args![provider.clone()]);
            assert!(card.status == 0, "that agent is retired");
            let buyer = CallerContext::transaction_signer_public_key();
            assert!(buyer != card.agent_key, "cannot open a lane to yourself");

            self.count += 1;
            let id = self.count;
            let token = ResourceManager::get(self.lanes).mint_non_fungible(
                NonFungibleId::from_u64(id),
                &LaneInfo { id, buyer, provider: provider.clone(), provider_key: card.agent_key, voucher_key, opened_epoch: Consensus::current_epoch() },
                &LaneState { deposited: amount, claimed: Amount::zero(), refunded: Amount::zero(), close_requested_epoch: None, closed: false },
            );
            if self.pages.last().expect("at least one page").balance() >= Amount::from(PAGE_SIZE) {
                self.pages.push(Vault::new_empty(self.lanes));
            }
            self.pages.last().expect("at least one page").deposit(token);
            emit_event(
                "lanes.lane_opened",
                metadata!(
                    "lane_id" => id.to_string(),
                    "buyer" => buyer.to_string(),
                    "provider" => provider,
                    "provider_key" => card.agent_key.to_string(),
                    "amount" => amount.to_string(),
                    "voucher_key" => voucher_key.to_string(),
                ),
            );
            id
        }

        /// Adds more escrow to one of the signer's lanes that isn't closing.
        pub fn fund_lane(&mut self, lane_id: u64, funds: Bucket) {
            let (info, mut state) = self.load(lane_id);
            assert!(info.buyer == CallerContext::transaction_signer_public_key(), "only the lane's buyer can fund it");
            assert!(!state.closed && state.close_requested_epoch.is_none(), "lane is closing or closed");
            let amount = self.take(funds);
            state.deposited = state.deposited.checked_add(amount).expect("lane deposit overflow");
            self.save(lane_id, &state);
            emit_event("lanes.lane_funded", metadata!("lane_id" => lane_id.to_string(), "amount" => amount.to_string()));
        }

        /// Pays the signer (the lane's provider key) for a voucher signed by the lane's voucher key. `spent_cum`
        /// is the cumulative amount the buyer has authorised on this lane; the provider receives the part not yet
        /// claimed. Claims still work while a close is pending, until the buyer refunds.
        pub fn claim(&mut self, lane_id: u64, spent_cum: Amount, buyer_signature: SignaturePayload) -> Bucket {
            let (info, mut state) = self.load(lane_id);
            assert!(info.provider_key == CallerContext::transaction_signer_public_key(), "only the lane's provider can claim");
            assert!(!state.closed, "lane is closed");
            assert!(spent_cum > state.claimed, "voucher does not exceed the amount already claimed");
            assert!(spent_cum <= state.deposited, "voucher exceeds the lane's escrow");

            let message = lane_voucher_message(&CallerContext::current_component_address(), lane_id, spent_cum);
            let verified = SignatureVerifier::with_domain(LANE_VOUCHER_DOMAIN).verify(
                &PublicKey::Ristretto25519(info.voucher_key),
                &message,
                &buyer_signature,
            );
            assert!(verified, "invalid voucher signature");

            let payout = spent_cum.checked_sub(state.claimed).expect("checked above");
            state.claimed = spent_cum;
            self.save(lane_id, &state);
            emit_event(
                "lanes.lane_claimed",
                metadata!("lane_id" => lane_id.to_string(), "spent_cum" => spent_cum.to_string(), "payout" => payout.to_string()),
            );
            self.escrow.withdraw(payout)
        }

        /// The buyer starts closing a lane. The provider can still claim until the close window has passed.
        pub fn request_close(&mut self, lane_id: u64) {
            let (info, mut state) = self.load(lane_id);
            assert!(info.buyer == CallerContext::transaction_signer_public_key(), "only the lane's buyer can close it");
            assert!(!state.closed, "lane is closed");
            assert!(state.close_requested_epoch.is_none(), "close was already requested");
            let epoch = Consensus::current_epoch();
            state.close_requested_epoch = Some(epoch);
            self.save(lane_id, &state);
            emit_event("lanes.close_requested", metadata!("lane_id" => lane_id.to_string(), "epoch" => epoch.to_string()));
        }

        /// The provider closes a lane at once; the buyer can then refund the remainder.
        pub fn release_lane(&mut self, lane_id: u64) {
            let (info, mut state) = self.load(lane_id);
            assert!(info.provider_key == CallerContext::transaction_signer_public_key(), "only the lane's provider can release it");
            assert!(!state.closed, "lane is closed");
            state.closed = true;
            self.save(lane_id, &state);
            emit_event("lanes.lane_released", metadata!("lane_id" => lane_id.to_string()));
        }

        /// The buyer takes back what the provider hasn't claimed, once the provider has released the lane or the
        /// close window has passed. The lane is closed afterwards.
        pub fn refund(&mut self, lane_id: u64) -> Bucket {
            let (info, mut state) = self.load(lane_id);
            assert!(info.buyer == CallerContext::transaction_signer_public_key(), "only the lane's buyer can refund it");
            if !state.closed {
                let requested = state.close_requested_epoch.expect("close was not requested");
                assert!(
                    Consensus::current_epoch() >= requested.saturating_add(self.lane_close_epochs),
                    "close window has not passed"
                );
            }
            let amount = state.remaining();
            assert!(amount.is_positive(), "nothing to refund");
            state.refunded = state.refunded.checked_add(amount).expect("refund overflow");
            state.closed = true;
            self.save(lane_id, &state);
            emit_event("lanes.lane_refunded", metadata!("lane_id" => lane_id.to_string(), "amount" => amount.to_string()));
            self.escrow.withdraw(amount)
        }

        // ---- reads ----

        /// (directory, payment resource, close window in epochs, lanes resource, lanes so far).
        pub fn config(&self) -> (ComponentAddress, ResourceAddress, u64, ResourceAddress, u64) {
            (self.directory, self.escrow.resource_address(), self.lane_close_epochs, self.lanes, self.count)
        }

        pub fn lane(&self, lane_id: u64) -> (LaneInfo, LaneState) {
            self.load(lane_id)
        }

        /// All escrow held for lanes.
        pub fn escrow(&self) -> Amount {
            self.escrow.balance()
        }

        // ---- internals ----

        fn take(&mut self, funds: Bucket) -> Amount {
            assert_eq!(funds.resource_address(), self.escrow.resource_address(), "wrong payment resource");
            let amount = funds.amount();
            assert!(amount.is_positive(), "lane amount must be positive");
            self.escrow.deposit(funds);
            amount
        }

        fn load(&self, lane_id: u64) -> (LaneInfo, LaneState) {
            assert!(lane_id >= 1 && lane_id <= self.count, "unknown lane");
            let nft = ResourceManager::get(self.lanes).get_non_fungible(&NonFungibleId::from_u64(lane_id));
            (nft.get_data(), nft.get_mutable_data())
        }

        fn save(&self, lane_id: u64, state: &LaneState) {
            ResourceManager::get(self.lanes).update_non_fungible_data(NonFungibleId::from_u64(lane_id), state);
        }
    }
}
