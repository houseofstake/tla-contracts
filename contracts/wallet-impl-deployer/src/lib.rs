use near_sdk::json_types::Base58CryptoHash;
use near_sdk::{
    env, near, require, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
};

const ON_DEPLOYED_GAS: Gas = Gas::from_tgas(10);
const GLOBAL_CODE_COST_PER_BYTE: u128 = 100_000_000_000_000_000_000;
/// Leased accounts reference the implementation by account id, so a publish
/// reaches every one of them at once. The delay gives owners a window to see
/// an approval and act on it before the code changes underneath them.
const DEFAULT_APPROVAL_DELAY_NS: u64 = 48 * 60 * 60 * 1_000_000_000;
const DEPLOY_LOCK_TTL_NS: u64 = 10 * 60 * 1_000_000_000;

mod error {
    pub const ONLY_COUNCIL: &str = "only council";
    pub const NO_APPROVED_HASH: &str = "no approved code hash";
    pub const HASH_MISMATCH: &str = "code does not match the approved hash";
    pub const DEPLOY_IN_FLIGHT: &str = "another deploy is in flight";
    pub const APPROVAL_TOO_YOUNG: &str = "approved code must wait out the delay before publishing";
    pub const INSUFFICIENT_DEPOSIT: &str = "attached deposit below global storage cost";
    pub const EMPTY_CODE: &str = "code must not be empty";
    pub const COST_OVERFLOW: &str = "storage cost overflow";
    pub const REQUIRES_ONE_YOCTO: &str = "requires an attached deposit of exactly 1 yoctoNEAR";
    pub const NO_APPROVED_UPGRADE: &str = "no approved upgrade hash";
    pub const UPGRADE_HASH_MISMATCH: &str = "code does not match the approved upgrade hash";
    pub const UPGRADE_TOO_YOUNG: &str =
        "an approved upgrade must wait out the delay before it installs";
}

#[near(event_json(standard = "hos_tla_impl_deployer"))]
pub enum Event {
    #[event_version("1.0.0")]
    HashApproved { hash: String, by: AccountId },
    #[event_version("1.0.0")]
    ImplDeployed { hash: String, size: u64 },
    #[event_version("1.0.0")]
    DeployFailed { hash: String },
    #[event_version("1.0.0")]
    SelfUpgraded {},
    #[event_version("1.0.0")]
    UpgradeApproved { hash: String, by: AccountId },
    #[event_version("1.0.0")]
    KeyDeleted { public_key: String, by: AccountId },
}

#[near(serializers = [json])]
pub struct DeployerView {
    pub council: AccountId,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct ImplDeployer {
    council: AccountId,
    current_hash: Option<[u8; 32]>,
    approved_hash: Option<[u8; 32]>,
    approved_at: Option<u64>,
    approval_delay_ns: u64,
    deploy_locked_until: u64,
    approved_upgrade_hash: Option<[u8; 32]>,
    approved_upgrade_at: Option<u64>,
}

#[near(serializers = [borsh])]
pub struct LegacyImplDeployer {
    council: AccountId,
    current_hash: Option<[u8; 32]>,
    approved_hash: Option<[u8; 32]>,
    approved_at: Option<u64>,
    approval_delay_ns: u64,
    deploy_in_flight: bool,
}

#[near]
impl ImplDeployer {
    #[private]
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        let Some(old) = hos_common::try_state_read::<LegacyImplDeployer>() else {
            return hos_common::try_state_read::<Self>()
                .unwrap_or_else(|| env::panic_str("no state to migrate"));
        };
        Self {
            council: old.council,
            current_hash: old.current_hash,
            approved_hash: None,
            approved_at: None,
            approval_delay_ns: old.approval_delay_ns,
            deploy_locked_until: 0,
            approved_upgrade_hash: None,
            approved_upgrade_at: None,
        }
    }

    #[init]
    /// `approval_delay_ns` is omitted in every real deployment, which takes the
    /// 48 hour default. The sandbox passes 0 so tests do not have to advance
    /// two days of blocks.
    pub fn new(council: AccountId, approval_delay_ns: Option<near_sdk::json_types::U64>) -> Self {
        Self {
            council,
            current_hash: None,
            approved_hash: None,
            approved_at: None,
            approval_delay_ns: approval_delay_ns
                .map(|value| value.0)
                .unwrap_or(DEFAULT_APPROVAL_DELAY_NS),
            deploy_locked_until: 0,
            approved_upgrade_hash: None,
            approved_upgrade_at: None,
        }
    }

    #[payable]
    pub fn gd_approve(&mut self, hash: Base58CryptoHash) {
        assert_one_yocto();
        let caller = env::predecessor_account_id();
        require!(caller == self.council, error::ONLY_COUNCIL);
        let raw: [u8; 32] = hash.into();
        self.approved_hash = Some(raw);
        self.approved_at = Some(env::block_timestamp());
        Event::HashApproved {
            hash: (&hash).into(),
            by: caller,
        }
        .emit();
    }

    #[payable]
    pub fn gd_deploy(&mut self, code: Vec<u8>) -> Promise {
        require!(
            env::block_timestamp() >= self.deploy_locked_until,
            error::DEPLOY_IN_FLIGHT
        );
        require!(!code.is_empty(), error::EMPTY_CODE);
        let approved = self
            .approved_hash
            .unwrap_or_else(|| env::panic_str(error::NO_APPROVED_HASH));
        require!(env::sha256_array(&code) == approved, error::HASH_MISMATCH);
        let approved_at = self
            .approved_at
            .unwrap_or_else(|| env::panic_str(error::NO_APPROVED_HASH));
        require!(
            env::block_timestamp() >= approved_at.saturating_add(self.approval_delay_ns),
            error::APPROVAL_TOO_YOUNG
        );
        let cost = (code.len() as u128)
            .checked_mul(GLOBAL_CODE_COST_PER_BYTE)
            .unwrap_or_else(|| env::panic_str(error::COST_OVERFLOW));
        let attached = env::attached_deposit().as_yoctonear();
        require!(attached >= cost, error::INSUFFICIENT_DEPOSIT);
        self.deploy_locked_until = env::block_timestamp().saturating_add(DEPLOY_LOCK_TTL_NS);
        let size = code.len() as u64;
        Promise::new(env::current_account_id())
            .deploy_global_contract_by_account_id(code)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(ON_DEPLOYED_GAS)
                    .gd_on_deployed(
                        Base58CryptoHash::from(approved),
                        size,
                        env::predecessor_account_id(),
                        env::attached_deposit(),
                        NearToken::from_yoctonear(cost),
                    ),
            )
    }

    #[private]
    pub fn gd_on_deployed(
        &mut self,
        hash: Base58CryptoHash,
        size: u64,
        payer: AccountId,
        attached: NearToken,
        cost: NearToken,
        #[callback_result] result: Result<(), PromiseError>,
    ) -> bool {
        self.deploy_locked_until = 0;
        match result {
            Ok(()) => {
                self.current_hash = Some(hash.into());
                self.approved_hash = None;
                self.approved_at = None;
                Event::ImplDeployed {
                    hash: (&hash).into(),
                    size,
                }
                .emit();
                let excess = attached.saturating_sub(cost);
                if excess > NearToken::ZERO {
                    let _ = Promise::new(payer).transfer(excess);
                }
                true
            }
            Err(_) => {
                Event::DeployFailed {
                    hash: (&hash).into(),
                }
                .emit();
                let _ = Promise::new(payer).transfer(attached);
                false
            }
        }
    }

    #[payable]
    pub fn approve_self_upgrade(&mut self, hash: Base58CryptoHash) {
        assert_one_yocto();
        let caller = env::predecessor_account_id();
        require!(caller == self.council, error::ONLY_COUNCIL);
        let raw: [u8; 32] = hash.into();
        self.approved_upgrade_hash = Some(raw);
        self.approved_upgrade_at = Some(env::block_timestamp());
        Event::UpgradeApproved {
            hash: (&hash).into(),
            by: caller,
        }
        .emit();
    }

    #[payable]
    pub fn upgrade_self(&mut self, code: near_sdk::json_types::Base64VecU8) -> Promise {
        assert_one_yocto();
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        let code = code.0;
        require!(!code.is_empty(), error::EMPTY_CODE);
        let approved = self
            .approved_upgrade_hash
            .unwrap_or_else(|| env::panic_str(error::NO_APPROVED_UPGRADE));
        require!(
            env::sha256_array(&code) == approved,
            error::UPGRADE_HASH_MISMATCH
        );
        let approved_at = self
            .approved_upgrade_at
            .unwrap_or_else(|| env::panic_str(error::NO_APPROVED_UPGRADE));
        require!(
            env::block_timestamp() >= approved_at.saturating_add(self.approval_delay_ns),
            error::UPGRADE_TOO_YOUNG
        );
        self.approved_upgrade_hash = None;
        self.approved_upgrade_at = None;
        Event::SelfUpgraded {}.emit();
        hos_common::deploy_and_migrate(code)
    }

    #[payable]
    pub fn gd_delete_key(&mut self, public_key: near_sdk::PublicKey) -> Promise {
        assert_one_yocto();
        let caller = env::predecessor_account_id();
        require!(caller == self.council, error::ONLY_COUNCIL);
        Event::KeyDeleted {
            public_key: String::from(&public_key),
            by: caller,
        }
        .emit();
        Promise::new(env::current_account_id()).delete_key(public_key)
    }

    pub fn approved_upgrade_hash(&self) -> Option<Base58CryptoHash> {
        self.approved_upgrade_hash.map(Base58CryptoHash::from)
    }

    pub fn approved_upgrade_at(&self) -> Option<near_sdk::json_types::U64> {
        self.approved_upgrade_at.map(near_sdk::json_types::U64)
    }

    pub fn current_hash(&self) -> Option<Base58CryptoHash> {
        self.current_hash.map(Base58CryptoHash::from)
    }

    pub fn approved_hash(&self) -> Option<Base58CryptoHash> {
        self.approved_hash.map(Base58CryptoHash::from)
    }

    pub fn approved_at(&self) -> Option<near_sdk::json_types::U64> {
        self.approved_at.map(near_sdk::json_types::U64)
    }

    pub fn approval_delay_ns(&self) -> near_sdk::json_types::U64 {
        near_sdk::json_types::U64(self.approval_delay_ns)
    }

    pub fn deploy_locked_until(&self) -> near_sdk::json_types::U64 {
        near_sdk::json_types::U64(self.deploy_locked_until)
    }

    pub fn config(&self) -> DeployerView {
        DeployerView {
            council: self.council.clone(),
        }
    }

    pub fn deploy_cost(&self, size: u64) -> NearToken {
        NearToken::from_yoctonear(
            u128::from(size)
                .checked_mul(GLOBAL_CODE_COST_PER_BYTE)
                .unwrap_or_else(|| env::panic_str(error::COST_OVERFLOW)),
        )
    }
}

fn assert_one_yocto() {
    require!(
        env::attached_deposit() == NearToken::from_yoctonear(1),
        error::REQUIRES_ONE_YOCTO
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;
    use std::str::FromStr;

    const IMPL: &str = "w.hos.testnet";
    const COUNCIL: &str = "council.testnet";
    const PATCH: &str = "patch.testnet";

    fn acc(s: &str) -> AccountId {
        AccountId::from_str(s).unwrap()
    }

    fn ctx(predecessor: &str, deposit: u128) {
        testing_env!(VMContextBuilder::new()
            .current_account_id(acc(IMPL))
            .predecessor_account_id(acc(predecessor))
            .attached_deposit(NearToken::from_yoctonear(deposit))
            .build());
    }

    const AFTER_DELAY: u64 = DEFAULT_APPROVAL_DELAY_NS + 1;

    fn ctx_at(predecessor: &str, deposit: u128, ts: u64) {
        testing_env!(VMContextBuilder::new()
            .current_account_id(acc(IMPL))
            .predecessor_account_id(acc(predecessor))
            .attached_deposit(NearToken::from_yoctonear(deposit))
            .block_timestamp(ts)
            .build());
    }

    fn deploy() -> ImplDeployer {
        ctx(COUNCIL, 0);
        ImplDeployer::new(acc(COUNCIL), None)
    }

    fn code() -> Vec<u8> {
        vec![7u8; 64]
    }

    fn code_hash() -> Base58CryptoHash {
        Base58CryptoHash::from(near_sdk::env::sha256_array(code()))
    }

    fn cost() -> u128 {
        64 * GLOBAL_CODE_COST_PER_BYTE
    }

    #[test]
    fn council_approves_and_anyone_deploys() {
        let mut c = deploy();
        ctx(COUNCIL, 1);
        c.gd_approve(code_hash());
        assert_eq!(c.approved_hash(), Some(code_hash()));
        ctx_at("anyone.testnet", cost(), AFTER_DELAY);
        let _ = c.gd_deploy(code());
        ctx(IMPL, 0);
        assert!(c.gd_on_deployed(
            code_hash(),
            64,
            acc("anyone.testnet"),
            NearToken::from_yoctonear(cost()),
            NearToken::from_yoctonear(cost()),
            Ok(()),
        ));
        assert_eq!(c.current_hash(), Some(code_hash()));
        assert_eq!(c.approved_hash(), None);
    }

    #[test]
    #[should_panic(expected = "only council")]
    fn a_second_approver_key_no_longer_exists() {
        let mut c = deploy();
        ctx(PATCH, 1);
        c.gd_approve(code_hash());
    }

    #[test]
    #[should_panic(expected = "only council")]
    fn outsider_cannot_approve() {
        let mut c = deploy();
        ctx("attacker.testnet", 1);
        c.gd_approve(code_hash());
    }

    #[test]
    #[should_panic(expected = "no approved code hash")]
    fn deploy_without_approval_rejected() {
        let mut c = deploy();
        ctx("anyone.testnet", cost());
        let _ = c.gd_deploy(code());
    }

    #[test]
    #[should_panic(expected = "code does not match the approved hash")]
    fn deploy_wrong_code_rejected() {
        let mut c = deploy();
        ctx(COUNCIL, 1);
        c.gd_approve(code_hash());
        ctx("anyone.testnet", cost());
        let _ = c.gd_deploy(vec![8u8; 64]);
    }

    #[test]
    #[should_panic(expected = "attached deposit below global storage cost")]
    fn deploy_underfunded_rejected() {
        let mut c = deploy();
        ctx(COUNCIL, 1);
        c.gd_approve(code_hash());
        ctx_at("anyone.testnet", cost() - 1, AFTER_DELAY);
        let _ = c.gd_deploy(code());
    }

    #[test]
    #[should_panic(expected = "approved code must wait out the delay before publishing")]
    fn deploy_before_the_delay_rejected() {
        let mut c = deploy();
        ctx(COUNCIL, 1);
        c.gd_approve(code_hash());
        ctx_at("anyone.testnet", cost(), DEFAULT_APPROVAL_DELAY_NS - 1);
        let _ = c.gd_deploy(code());
    }

    #[test]
    #[should_panic(expected = "another deploy is in flight")]
    fn concurrent_deploy_rejected() {
        let mut c = deploy();
        ctx(COUNCIL, 1);
        c.gd_approve(code_hash());
        ctx_at("anyone.testnet", cost(), AFTER_DELAY);
        let _ = c.gd_deploy(code());
        ctx_at("anyone.testnet", cost(), AFTER_DELAY);
        let _ = c.gd_deploy(code());
    }

    const DEPLOYED_STATE_B64: &str = "FwAAAGNvdW5jaWwuaG9zZGVtby50ZXN0bmV0AWkbd5BFQct9Clizk9gSgdfUyr4FqFoqjdtIqm5dR/SjAYOB4845uPXNPuR7lEb/iUVng0q6TpN7AzBTKeYTlF9PAbHLBUX3oskYAACeIimdAAAA";

    #[test]
    fn the_legacy_struct_still_matches_the_deployed_state() {
        use near_sdk::base64::Engine;
        use near_sdk::borsh::BorshDeserialize;
        let raw = near_sdk::base64::engine::general_purpose::STANDARD
            .decode(DEPLOYED_STATE_B64)
            .expect("fixture is valid base64");
        let old = LegacyImplDeployer::try_from_slice(&raw)
            .expect("deployed state no longer decodes as LegacyImplDeployer");
        assert_eq!(old.council.as_str(), "council.hosdemo.testnet");
        assert_eq!(old.approval_delay_ns, DEFAULT_APPROVAL_DELAY_NS);
        assert!(old.current_hash.is_some());
        assert!(!old.deploy_in_flight);
    }

    #[test]
    fn a_deploy_whose_callback_never_lands_stops_blocking_after_the_ttl() {
        let mut c = deploy();
        ctx(COUNCIL, 1);
        c.gd_approve(code_hash());
        ctx_at("anyone.testnet", cost(), AFTER_DELAY);
        let _ = c.gd_deploy(code());
        ctx_at("anyone.testnet", cost(), AFTER_DELAY + DEPLOY_LOCK_TTL_NS);
        let _ = c.gd_deploy(code());
        assert_eq!(
            c.deploy_locked_until().0,
            AFTER_DELAY + 2 * DEPLOY_LOCK_TTL_NS
        );
    }

    #[test]
    fn failed_deploy_clears_flight_and_keeps_approval_consumable() {
        let mut c = deploy();
        ctx(COUNCIL, 1);
        c.gd_approve(code_hash());
        ctx_at("anyone.testnet", cost(), AFTER_DELAY);
        let _ = c.gd_deploy(code());
        ctx(IMPL, 0);
        assert!(!c.gd_on_deployed(
            code_hash(),
            64,
            acc("anyone.testnet"),
            NearToken::from_yoctonear(cost()),
            NearToken::from_yoctonear(cost()),
            Err(PromiseError::Failed),
        ));
        assert_eq!(c.current_hash(), None);
        assert_eq!(c.approved_hash(), Some(code_hash()));
        ctx_at("anyone.testnet", cost(), AFTER_DELAY);
        let _ = c.gd_deploy(code());
    }

    #[test]
    #[should_panic(expected = "only council")]
    fn self_upgrade_rejects_a_non_council_caller() {
        let mut c = deploy();
        ctx(PATCH, 1);
        let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(code()));
    }

    #[test]
    #[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
    fn approval_rejects_a_restricted_access_key() {
        let mut c = deploy();
        ctx(COUNCIL, 0);
        c.gd_approve(code_hash());
    }

    #[test]
    #[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
    fn self_upgrade_rejects_a_restricted_access_key() {
        let mut c = deploy();
        ctx(COUNCIL, 0);
        let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(code()));
    }

    #[test]
    fn deploy_cost_is_linear() {
        let c = deploy();
        assert_eq!(
            c.deploy_cost(200_000).as_yoctonear(),
            200_000u128 * GLOBAL_CODE_COST_PER_BYTE
        );
    }

    #[test]
    #[should_panic(expected = "no approved upgrade hash")]
    fn a_self_upgrade_without_an_approval_is_refused() {
        let mut c = deploy();
        ctx(COUNCIL, 1);
        let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(code()));
    }

    #[test]
    #[should_panic(expected = "must wait out the delay")]
    fn a_self_upgrade_inside_the_window_is_refused() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        c.approve_self_upgrade(code_hash());
        ctx_at(COUNCIL, 1, DEFAULT_APPROVAL_DELAY_NS - 1);
        let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(code()));
    }

    #[test]
    #[should_panic(expected = "does not match the approved upgrade hash")]
    fn a_self_upgrade_of_different_code_is_refused() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        c.approve_self_upgrade(code_hash());
        ctx_at(COUNCIL, 1, AFTER_DELAY);
        let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(vec![9u8; 64]));
    }

    #[test]
    fn an_approved_self_upgrade_installs_once_the_window_passes() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        c.approve_self_upgrade(code_hash());
        assert_eq!(c.approved_upgrade_hash(), Some(code_hash()));
        ctx_at(COUNCIL, 1, AFTER_DELAY);
        let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(code()));
        assert!(
            c.approved_upgrade_hash().is_none(),
            "the approval is spent by the upgrade it authorised"
        );
    }

    #[test]
    #[should_panic(expected = "only council")]
    fn only_the_council_may_remove_a_key() {
        let mut c = deploy();
        ctx("anyone.testnet", 1);
        let _ = c.gd_delete_key(
            "ed25519:6E8sCci9badyRkXb3JoRpBj5p8C6Tw41ELDZoiihKEtp"
                .parse()
                .unwrap(),
        );
    }

    #[test]
    #[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
    fn removing_a_key_needs_a_full_access_signature() {
        let mut c = deploy();
        ctx(COUNCIL, 0);
        let _ = c.gd_delete_key(
            "ed25519:6E8sCci9badyRkXb3JoRpBj5p8C6Tw41ELDZoiihKEtp"
                .parse()
                .unwrap(),
        );
    }

    #[test]
    fn the_legacy_arm_runs_and_drops_a_stuck_deploy_flag() {
        ctx(COUNCIL, 0);
        env::state_write(&LegacyImplDeployer {
            council: acc(COUNCIL),
            current_hash: Some([3u8; 32]),
            approved_hash: Some([4u8; 32]),
            approved_at: Some(7),
            approval_delay_ns: 11,
            deploy_in_flight: true,
        });
        let migrated = ImplDeployer::migrate();
        assert_eq!(migrated.council, acc(COUNCIL));
        assert_eq!(migrated.current_hash, Some([3u8; 32]));
        assert_eq!(migrated.approval_delay_ns, 11);
        assert_eq!(
            migrated.deploy_locked_until, 0,
            "a publish stuck in flight must not carry across an upgrade"
        );
        assert!(
            migrated.approved_hash.is_none(),
            "an approval must not survive the code it was granted against"
        );
    }

    #[test]
    fn state_already_current_survives_a_same_shape_redeploy() {
        ctx(COUNCIL, 0);
        let current = deploy();
        let delay = current.approval_delay_ns;
        env::state_write(&current);
        assert_eq!(ImplDeployer::migrate().approval_delay_ns, delay);
    }

    #[test]
    #[should_panic(expected = "no state to migrate")]
    fn an_account_with_no_state_refuses_rather_than_writing_a_default() {
        ctx(COUNCIL, 0);
        let _ = ImplDeployer::migrate();
    }
}
