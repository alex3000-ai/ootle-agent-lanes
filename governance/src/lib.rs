//! LanesGovernance: the network's admin-controlled settings.
//!
//! Creating the component mints a single admin badge; every setter requires a proof of it. Reads are
//! public, and `LaneLedger` reads `ledger_config` on each lane operation.

use tari_template_lib::prelude::*;

/// Hard cap on the operator share of every claim (15%).
const MAX_FEE_BPS: u16 = 1_500;

#[template]
mod lanes_governance {
    use super::*;

    pub struct LanesGovernance {
        admin_badge: ResourceAddress,
        /// Hash of the network rules buyers consent to when they open a lane.
        rules_ver: u32,
        rules_hash: Hash32,
        /// Operator share of every lane claim, in basis points.
        fee_bps: u16,
        /// Epochs a provider has to submit its latest voucher after a buyer asks to close a lane.
        lane_close_epochs: u64,
        paused: bool,
    }

    impl LanesGovernance {
        /// Creates the governance component and returns it with the admin badge.
        pub fn new(fee_bps: u16, lane_close_epochs: u64) -> (Component<Self>, Bucket) {
            assert_fee_bps(fee_bps);
            assert!(lane_close_epochs > 0, "lane_close_epochs must be positive");

            let badge = ResourceBuilder::public_fungible()
                .with_token_symbol("LANESADMIN")
                .initial_supply(1u64);
            let admin = rule!(resource(badge.resource_address()));
            let access_rules = AccessRules::new()
                .default(rule!(allow_all))
                .method("set_rules", admin.clone())
                .method("set_params", admin.clone())
                .method("set_paused", admin);

            let component = Component::new(Self {
                admin_badge: badge.resource_address(),
                rules_ver: 0,
                rules_hash: Hash32::default(),
                fee_bps,
                lane_close_epochs,
                paused: false,
            })
            .with_access_rules(access_rules)
            .with_owner_rule(OwnerRule::None)
            .create();
            (component, badge)
        }

        pub fn set_rules(&mut self, ver: u32, hash: Hash32) {
            assert!(ver > self.rules_ver, "rules version must increase");
            self.rules_ver = ver;
            self.rules_hash = hash;
            emit_event("lanes.rules", metadata!("ver" => ver.to_string(), "hash" => hash.to_string()));
        }

        pub fn set_params(&mut self, fee_bps: u16, lane_close_epochs: u64) {
            assert_fee_bps(fee_bps);
            assert!(lane_close_epochs > 0, "lane_close_epochs must be positive");
            self.fee_bps = fee_bps;
            self.lane_close_epochs = lane_close_epochs;
            emit_event(
                "lanes.params",
                metadata!(
                    "fee_bps" => fee_bps.to_string(),
                    "lane_close_epochs" => lane_close_epochs.to_string(),
                ),
            );
        }

        pub fn set_paused(&mut self, paused: bool) {
            self.paused = paused;
            emit_event("lanes.paused", metadata!("paused" => paused.to_string()));
        }

        pub fn admin_badge(&self) -> ResourceAddress {
            self.admin_badge
        }

        pub fn rules(&self) -> (u32, Hash32) {
            (self.rules_ver, self.rules_hash)
        }

        /// What `LaneLedger` needs on every lane operation:
        /// `(fee_bps, lane_close_epochs, rules_hash, paused)`.
        pub fn ledger_config(&self) -> (u16, u64, Hash32, bool) {
            (self.fee_bps, self.lane_close_epochs, self.rules_hash, self.paused)
        }
    }
}

fn assert_fee_bps(fee_bps: u16) {
    assert!(fee_bps <= MAX_FEE_BPS, "fee_bps exceeds the 15% cap");
}
