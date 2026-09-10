use crate::ImplDeployer;
use near_sdk::{near, AccountId};

#[near(serializers = [borsh])]
pub struct ImplDeployerV1 {
    pub state_version: u16,
    pub council: AccountId,
    pub current_hash: Option<[u8; 32]>,
    pub approved_hash: Option<[u8; 32]>,
    pub approved_at: Option<u64>,
    pub approval_delay_ns: u64,
    pub deploy_locked_until: u64,
    pub approved_upgrade_hash: Option<[u8; 32]>,
    pub approved_upgrade_at: Option<u64>,
    pub upgrade_proven: bool,
}

impl From<ImplDeployerV1> for ImplDeployer {
    fn from(old: ImplDeployerV1) -> Self {
        Self {
            state_version: crate::STATE_VERSION,
            council: old.council,
            current_hash: old.current_hash,
            approved_hash: old.approved_hash,
            approved_at: old.approved_at,
            approval_delay_ns: old.approval_delay_ns,
            deploy_locked_until: old.deploy_locked_until,
            approved_upgrade_hash: old.approved_upgrade_hash,
            approved_upgrade_at: old.approved_upgrade_at,
            upgrade_proven: old.upgrade_proven,
            pending_council: None,
            pending_council_at: None,
        }
    }
}
