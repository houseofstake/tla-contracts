use near_sdk::json_types::Base58CryptoHash;
use near_sdk::{
    env, near, require, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
};

const ON_DEPLOYED_GAS: Gas = Gas::from_tgas(10);
const GLOBAL_CODE_COST_PER_BYTE: u128 = 100_000_000_000_000_000_000;

mod error {
    pub const ONLY_COUNCIL: &str = "only council";
    pub const ONLY_APPROVER: &str = "only council or patch authority";
    pub const NO_APPROVED_HASH: &str = "no approved code hash";
    pub const HASH_MISMATCH: &str = "code does not match the approved hash";
    pub const DEPLOY_IN_FLIGHT: &str = "another deploy is in flight";
    pub const INSUFFICIENT_DEPOSIT: &str = "attached deposit below global storage cost";
    pub const EMPTY_CODE: &str = "code must not be empty";
    pub const COST_OVERFLOW: &str = "storage cost overflow";
    pub const REQUIRES_ONE_YOCTO: &str = "requires an attached deposit of exactly 1 yoctoNEAR";
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
}

#[near(serializers = [json])]
pub struct DeployerView {
    pub council: AccountId,
    pub patch_authority: AccountId,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct ImplDeployer {
    council: AccountId,
    patch_authority: AccountId,
    current_hash: Option<[u8; 32]>,
    approved_hash: Option<[u8; 32]>,
    deploy_in_flight: bool,
}

#[near]
impl ImplDeployer {
    #[init]
    pub fn new(council: AccountId, patch_authority: AccountId) -> Self {
        Self {
            council,
            patch_authority,
            current_hash: None,
            approved_hash: None,
            deploy_in_flight: false,
        }
    }

    #[payable]
    pub fn gd_approve(&mut self, hash: Base58CryptoHash) {
        assert_one_yocto();
        let caller = env::predecessor_account_id();
        require!(
            caller == self.council || caller == self.patch_authority,
            error::ONLY_APPROVER
        );
        let raw: [u8; 32] = hash.into();
        self.approved_hash = Some(raw);
        Event::HashApproved {
            hash: (&hash).into(),
            by: caller,
        }
        .emit();
    }

    #[payable]
    pub fn gd_deploy(&mut self, code: Vec<u8>) -> Promise {
        require!(!self.deploy_in_flight, error::DEPLOY_IN_FLIGHT);
        require!(!code.is_empty(), error::EMPTY_CODE);
        let approved = self
            .approved_hash
            .unwrap_or_else(|| env::panic_str(error::NO_APPROVED_HASH));
        require!(env::sha256_array(&code) == approved, error::HASH_MISMATCH);
        let cost = (code.len() as u128)
            .checked_mul(GLOBAL_CODE_COST_PER_BYTE)
            .unwrap_or_else(|| env::panic_str(error::COST_OVERFLOW));
        let attached = env::attached_deposit().as_yoctonear();
        require!(attached >= cost, error::INSUFFICIENT_DEPOSIT);
        self.deploy_in_flight = true;
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
        self.deploy_in_flight = false;
        match result {
            Ok(()) => {
                self.current_hash = Some(hash.into());
                self.approved_hash = None;
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
    pub fn upgrade_self(&mut self, code: Vec<u8>) -> Promise {
        assert_one_yocto();
        require!(
            env::predecessor_account_id() == self.council,
            error::ONLY_COUNCIL
        );
        require!(!code.is_empty(), error::EMPTY_CODE);
        Event::SelfUpgraded {}.emit();
        Promise::new(env::current_account_id()).deploy_contract(code)
    }

    pub fn current_hash(&self) -> Option<Base58CryptoHash> {
        self.current_hash.map(Base58CryptoHash::from)
    }

    pub fn approved_hash(&self) -> Option<Base58CryptoHash> {
        self.approved_hash.map(Base58CryptoHash::from)
    }

    pub fn config(&self) -> DeployerView {
        DeployerView {
            council: self.council.clone(),
            patch_authority: self.patch_authority.clone(),
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

    fn deploy() -> ImplDeployer {
        ctx(COUNCIL, 0);
        ImplDeployer::new(acc(COUNCIL), acc(PATCH))
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
        ctx("anyone.testnet", cost());
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
    fn patch_authority_can_approve() {
        let mut c = deploy();
        ctx(PATCH, 1);
        c.gd_approve(code_hash());
        assert_eq!(c.approved_hash(), Some(code_hash()));
    }

    #[test]
    #[should_panic(expected = "only council or patch authority")]
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
        ctx("anyone.testnet", cost() - 1);
        let _ = c.gd_deploy(code());
    }

    #[test]
    #[should_panic(expected = "another deploy is in flight")]
    fn concurrent_deploy_rejected() {
        let mut c = deploy();
        ctx(COUNCIL, 1);
        c.gd_approve(code_hash());
        ctx("anyone.testnet", cost());
        let _ = c.gd_deploy(code());
        ctx("anyone.testnet", cost());
        let _ = c.gd_deploy(code());
    }

    #[test]
    fn failed_deploy_clears_flight_and_keeps_approval_consumable() {
        let mut c = deploy();
        ctx(COUNCIL, 1);
        c.gd_approve(code_hash());
        ctx("anyone.testnet", cost());
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
        ctx("anyone.testnet", cost());
        let _ = c.gd_deploy(code());
    }

    #[test]
    #[should_panic(expected = "only council")]
    fn self_upgrade_rejects_patch_authority() {
        let mut c = deploy();
        ctx(PATCH, 1);
        let _ = c.upgrade_self(code());
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
        let _ = c.upgrade_self(code());
    }

    #[test]
    fn deploy_cost_is_linear() {
        let c = deploy();
        assert_eq!(
            c.deploy_cost(200_000).as_yoctonear(),
            200_000u128 * GLOBAL_CODE_COST_PER_BYTE
        );
    }
}
