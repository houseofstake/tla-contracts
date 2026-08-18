mod error;
mod events;

use near_sdk::json_types::{Base58CryptoHash, Base64VecU8};
use near_sdk::{
    env, near, require, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
};

use crate::events::Event;

const ON_DEPLOYED_GAS: Gas = Gas::from_tgas(10);
const GLOBAL_CODE_COST_PER_BYTE: u128 = 100_000_000_000_000_000_000;
/// Leased accounts reference the implementation by account id, so a publish
/// reaches every one of them at once. The delay gives owners a window to see
/// an approval and act on it before the code changes underneath them.
const DEFAULT_APPROVAL_DELAY_NS: u64 = 48 * 60 * 60 * 1_000_000_000;
const DEPLOY_LOCK_TTL_NS: u64 = 10 * 60 * 1_000_000_000;

#[near(serializers = [json])]
pub struct DeployerView {
    pub council: AccountId,
    pub approval_delay_ns: near_sdk::json_types::U64,
    pub production_delay: bool,
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
    deploy_locked_until: u64,
    approved_upgrade_hash: Option<[u8; 32]>,
    approved_upgrade_at: Option<u64>,
}

#[near]
impl ImplDeployer {
    #[private]
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        Event::SelfUpgraded {}.emit();
        let Some(old) = hos_common::try_state_read::<LegacyImplDeployer>() else {
            return hos_common::try_state_read::<Self>()
                .unwrap_or_else(|| env::panic_str(error::NO_STATE));
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
    /// Omitting `approval_delay_ns` takes the 48 hour default. Passing zero
    /// removes the window owners have to see an approval before their code
    /// changes, which suits a sandbox and nothing else, so `config` reports
    /// whether the deployed value is production grade.
    pub fn new(council: AccountId, approval_delay_ns: Option<near_sdk::json_types::U64>) -> Self {
        require!(council != env::current_account_id(), error::COUNCIL_IS_SELF);
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
    pub fn gd_deploy(&mut self, code: Base64VecU8) -> Promise {
        let code: Vec<u8> = code.into();
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
            approval_delay_ns: near_sdk::json_types::U64(self.approval_delay_ns),
            production_delay: self.approval_delay_ns >= DEFAULT_APPROVAL_DELAY_NS,
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
mod tests;
