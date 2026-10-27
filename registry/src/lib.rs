//! AgentRegistry: service providers and buyer profiles.
//!
//! Providers act for themselves: every provider method is keyed by the transaction signer's public
//! key, so nobody can register, relist or change policy on another provider's behalf. The admin
//! (holder of the governance badge) can only ban providers and appoint attesters.
//!
//! Buyer profiles carry one of two tags, and neither needs a human to create:
//! - `own-wallet`: the default. Any key can register itself, including an autonomous agent that
//!   created its own wallet. A management agent can link sub-agents under it (both keys sign).
//! - `human-verified`: an attester appointed by the admin vouches that a human stands behind the
//!   wallet, until an expiry epoch or until revoked.
//!
//! Each provider picks which buyers may open lanes to it (`BuyerPolicy`). Buyers need no profile
//! to pay a provider with the default `Anyone` policy.
//!
//! State lives in maps on the component. That is fine at testnet scale; per-entry substates are a
//! later optimisation once real provider counts are known.

use std::collections::{BTreeMap, BTreeSet};

use minicbor::{CborLen, Decode, Encode};
use tari_template_lib::prelude::*;

/// Profile tag for a wallet that owns itself (agent or human, unverified).
pub const TAG_OWN_WALLET: &str = "own-wallet";
/// Profile tag for a wallet with a live human attestation.
pub const TAG_HUMAN_VERIFIED: &str = "human-verified";
/// How many parent links `human_verified_root` follows before giving up.
pub const MAX_PARENT_DEPTH: usize = 8;
/// Longest listing a provider can publish (a URL or a short service description).
pub const MAX_LISTING_LEN: usize = 256;

/// Buyer policy codes, as passed to `register_provider` and `set_buyer_policy`.
pub const POLICY_ANYONE: u8 = 0;
/// The buyer, or a management agent it works under, is human-verified.
pub const POLICY_HUMAN_ROOT: u8 = 1;
/// The buyer's own wallet is human-verified.
pub const POLICY_HUMAN_VERIFIED: u8 = 2;

#[derive(Debug, Clone, Encode, Decode, CborLen)]
pub struct Provider {
    #[n(0)]
    pub registered_epoch: u64,
    #[n(1)]
    pub banned: bool,
    /// Where buyers find out what this provider sells (for example an Ootlebook service page).
    #[n(2)]
    pub listing: String,
    /// One of the `POLICY_*` codes.
    #[n(3)]
    pub buyer_policy: u8,
}

#[derive(Debug, Clone, Encode, Decode, CborLen)]
pub struct HumanAttestation {
    #[n(0)]
    pub attester: RistrettoPublicKeyBytes,
    /// Hash of the attester's off-chain evidence (never the evidence itself).
    #[n(1)]
    pub evidence_hash: Hash32,
    /// The attestation lapses at this epoch.
    #[n(2)]
    pub expires_epoch: u64,
}

#[derive(Debug, Clone, Encode, Decode, CborLen)]
pub struct Profile {
    #[n(0)]
    pub registered_epoch: u64,
    /// The management agent this wallet works under, once both sides have signed.
    #[n(1)]
    pub parent: Option<RistrettoPublicKeyBytes>,
    /// A parent the profile asked for that has not confirmed yet.
    #[n(2)]
    pub pending_parent: Option<RistrettoPublicKeyBytes>,
    #[n(3)]
    pub human: Option<HumanAttestation>,
}

impl Profile {
    fn human_verified_at(&self, epoch: u64) -> bool {
        self.human.as_ref().is_some_and(|attestation| attestation.expires_epoch > epoch)
    }
}

#[template]
mod agent_registry {
    use super::*;

    pub struct AgentRegistry {
        providers: BTreeMap<RistrettoPublicKeyBytes, Provider>,
        profiles: BTreeMap<RistrettoPublicKeyBytes, Profile>,
        attesters: BTreeSet<RistrettoPublicKeyBytes>,
    }

    impl AgentRegistry {
        /// `admin_badge` is the badge minted by `LanesGovernance::new`.
        pub fn new(admin_badge: ResourceAddress) -> Component<Self> {
            let admin = rule!(resource(admin_badge));
            let access_rules = AccessRules::new()
                .default(rule!(allow_all))
                .method("ban_provider", admin.clone())
                .method("unban_provider", admin.clone())
                .method("add_attester", admin.clone())
                .method("remove_attester", admin);
            Component::new(Self {
                providers: BTreeMap::new(),
                profiles: BTreeMap::new(),
                attesters: BTreeSet::new(),
            })
            .with_access_rules(access_rules)
            .with_owner_rule(OwnerRule::None)
            .create()
        }

        // ---- admin ----

        pub fn ban_provider(&mut self, provider: RistrettoPublicKeyBytes) {
            self.providers.get_mut(&provider).expect("unknown provider").banned = true;
            emit_event("lanes.provider_banned", metadata!("provider" => provider.to_string(), "banned" => "true"));
        }

        pub fn unban_provider(&mut self, provider: RistrettoPublicKeyBytes) {
            self.providers.get_mut(&provider).expect("unknown provider").banned = false;
            emit_event("lanes.provider_banned", metadata!("provider" => provider.to_string(), "banned" => "false"));
        }

        pub fn add_attester(&mut self, attester: RistrettoPublicKeyBytes) {
            self.attesters.insert(attester);
            emit_event("lanes.attester", metadata!("attester" => attester.to_string(), "active" => "true"));
        }

        /// Existing attestations stay until they expire or are revoked; the attester just can't
        /// issue new ones.
        pub fn remove_attester(&mut self, attester: RistrettoPublicKeyBytes) {
            self.attesters.remove(&attester);
            emit_event("lanes.attester", metadata!("attester" => attester.to_string(), "active" => "false"));
        }

        // ---- providers (keyed by the transaction signer) ----

        pub fn register_provider(&mut self, listing: String, buyer_policy: u8) {
            let provider = CallerContext::transaction_signer_public_key();
            assert!(!self.providers.contains_key(&provider), "provider already registered");
            assert_listing(&listing);
            assert_policy(buyer_policy);
            self.providers.insert(provider, Provider {
                registered_epoch: Consensus::current_epoch(),
                banned: false,
                listing: listing.clone(),
                buyer_policy,
            });
            emit_event(
                "lanes.provider_registered",
                metadata!("provider" => provider.to_string(), "listing" => listing, "buyer_policy" => buyer_policy.to_string()),
            );
        }

        pub fn set_listing(&mut self, listing: String) {
            assert_listing(&listing);
            let provider = CallerContext::transaction_signer_public_key();
            self.signer_provider().listing = listing.clone();
            emit_event("lanes.provider_listing", metadata!("provider" => provider.to_string(), "listing" => listing));
        }

        /// Applies to lanes opened from now on; open lanes are unaffected.
        pub fn set_buyer_policy(&mut self, buyer_policy: u8) {
            assert_policy(buyer_policy);
            let provider = CallerContext::transaction_signer_public_key();
            self.signer_provider().buyer_policy = buyer_policy;
            emit_event(
                "lanes.provider_policy",
                metadata!("provider" => provider.to_string(), "buyer_policy" => buyer_policy.to_string()),
            );
        }

        // ---- buyer profiles (keyed by the transaction signer) ----

        /// Registers the signer's profile with the `own-wallet` tag. With `parent`, the profile asks
        /// to work under that management agent; the link holds once the parent calls
        /// `confirm_sub_agent`.
        pub fn register_profile(&mut self, parent: Option<RistrettoPublicKeyBytes>) {
            let subject = CallerContext::transaction_signer_public_key();
            assert!(!self.profiles.contains_key(&subject), "profile already registered");
            if let Some(parent) = parent {
                assert!(parent != subject, "a profile cannot be its own parent");
                assert!(self.profiles.contains_key(&parent), "parent has no profile");
            }
            self.profiles.insert(subject, Profile {
                registered_epoch: Consensus::current_epoch(),
                parent: None,
                pending_parent: parent,
                human: None,
            });
            emit_event("lanes.profile_registered", metadata!("profile" => subject.to_string()));
        }

        /// The signer asks to work under `parent` (replacing any pending request). The current
        /// link, if any, stays until the new parent confirms.
        pub fn request_parent(&mut self, parent: RistrettoPublicKeyBytes) {
            let subject = CallerContext::transaction_signer_public_key();
            assert!(parent != subject, "a profile cannot be its own parent");
            assert!(self.profiles.contains_key(&parent), "parent has no profile");
            self.profiles.get_mut(&subject).expect("signer has no profile").pending_parent = Some(parent);
        }

        /// The signer (a management agent) accepts `child` as its sub-agent.
        pub fn confirm_sub_agent(&mut self, child: RistrettoPublicKeyBytes) {
            let parent = CallerContext::transaction_signer_public_key();
            assert!(self.profiles.contains_key(&parent), "signer has no profile");
            let record = self.profiles.get_mut(&child).expect("child has no profile");
            assert!(record.pending_parent == Some(parent), "child did not ask for this parent");
            record.parent = Some(parent);
            record.pending_parent = None;
            emit_event("lanes.sub_agent", metadata!("parent" => parent.to_string(), "child" => child.to_string(), "linked" => "true"));
        }

        /// Either side ends a sub-agent link: the child itself, or its parent.
        pub fn detach_sub_agent(&mut self, child: RistrettoPublicKeyBytes) {
            let signer = CallerContext::transaction_signer_public_key();
            let record = self.profiles.get_mut(&child).expect("child has no profile");
            let parent = record.parent.expect("profile has no parent");
            assert!(signer == child || signer == parent, "only the child or its parent can detach");
            record.parent = None;
            emit_event("lanes.sub_agent", metadata!("parent" => parent.to_string(), "child" => child.to_string(), "linked" => "false"));
        }

        /// An attester vouches that a human stands behind `subject` until `expires_epoch`.
        pub fn attest_human(&mut self, subject: RistrettoPublicKeyBytes, evidence_hash: Hash32, expires_epoch: u64) {
            let attester = CallerContext::transaction_signer_public_key();
            assert!(self.attesters.contains(&attester), "signer is not an attester");
            assert!(expires_epoch > Consensus::current_epoch(), "attestation must expire in the future");
            let record = self.profiles.get_mut(&subject).expect("subject has no profile");
            record.human = Some(HumanAttestation { attester, evidence_hash, expires_epoch });
            emit_event("lanes.human_attested", metadata!("profile" => subject.to_string(), "attester" => attester.to_string()));
        }

        /// Any current attester, or the subject itself, can drop a human attestation.
        pub fn revoke_human(&mut self, subject: RistrettoPublicKeyBytes) {
            let signer = CallerContext::transaction_signer_public_key();
            assert!(signer == subject || self.attesters.contains(&signer), "only an attester or the subject can revoke");
            let record = self.profiles.get_mut(&subject).expect("subject has no profile");
            assert!(record.human.take().is_some(), "profile has no human attestation");
            emit_event("lanes.human_revoked", metadata!("profile" => subject.to_string()));
        }

        // ---- reads ----

        pub fn profile(&self, subject: RistrettoPublicKeyBytes) -> Option<Profile> {
            self.profiles.get(&subject).cloned()
        }

        /// `own-wallet` or `human-verified`, or `None` without a profile.
        pub fn profile_tag(&self, subject: RistrettoPublicKeyBytes) -> Option<String> {
            let epoch = Consensus::current_epoch();
            self.profiles.get(&subject).map(|record| {
                if record.human_verified_at(epoch) { TAG_HUMAN_VERIFIED } else { TAG_OWN_WALLET }.to_string()
            })
        }

        /// True when `subject` or one of its confirmed parents (up to `MAX_PARENT_DEPTH` links) is
        /// human-verified. Lets a provider tell an agent that ultimately answers to a human.
        pub fn human_verified_root(&self, subject: RistrettoPublicKeyBytes) -> bool {
            let epoch = Consensus::current_epoch();
            let mut current = Some(subject);
            for _ in 0..=MAX_PARENT_DEPTH {
                let Some(record) = current.and_then(|key| self.profiles.get(&key)) else {
                    return false;
                };
                if record.human_verified_at(epoch) {
                    return true;
                }
                current = record.parent;
            }
            false
        }

        /// Whether `buyer` satisfies `provider`'s buyer policy. False for unknown or banned
        /// providers.
        pub fn buyer_allowed(&self, provider: RistrettoPublicKeyBytes, buyer: RistrettoPublicKeyBytes) -> bool {
            let Some(record) = self.providers.get(&provider) else {
                return false;
            };
            if record.banned {
                return false;
            }
            match record.buyer_policy {
                POLICY_ANYONE => true,
                POLICY_HUMAN_ROOT => self.human_verified_root(buyer),
                _ => {
                    let epoch = Consensus::current_epoch();
                    self.profiles.get(&buyer).is_some_and(|profile| profile.human_verified_at(epoch))
                },
            }
        }

        pub fn is_attester(&self, attester: RistrettoPublicKeyBytes) -> bool {
            self.attesters.contains(&attester)
        }

        pub fn is_active_provider(&self, provider: RistrettoPublicKeyBytes) -> bool {
            self.providers.get(&provider).is_some_and(|record| !record.banned)
        }

        pub fn provider(&self, provider: RistrettoPublicKeyBytes) -> Option<Provider> {
            self.providers.get(&provider).cloned()
        }

        fn signer_provider(&mut self) -> &mut Provider {
            let provider = CallerContext::transaction_signer_public_key();
            self.providers.get_mut(&provider).expect("provider not registered")
        }
    }
}

fn assert_listing(listing: &str) {
    assert!(listing.len() <= MAX_LISTING_LEN, "listing must be at most 256 bytes");
}

fn assert_policy(buyer_policy: u8) {
    assert!(buyer_policy <= POLICY_HUMAN_VERIFIED, "unknown buyer policy");
}
