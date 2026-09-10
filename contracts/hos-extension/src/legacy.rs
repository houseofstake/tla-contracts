use crate::HosExtension;
use near_sdk::near;
use near_sdk::store::IterableSet;
use near_sdk::AccountId;

#[near(serializers = [borsh])]
pub struct HosExtensionV1 {
    pub state_version: u16,
    pub admins: IterableSet<AccountId>,
    pub registry: AccountId,
    pub recovery: AccountId,
    pub paused: bool,
    pub version: u8,
    pub treasury: AccountId,
    pub approved_code_hash: Option<[u8; 32]>,
    pub approved_at: Option<u64>,
    pub council: AccountId,
    pub paused_until_ns: u64,
    pub recovery_reset_pending: IterableSet<AccountId>,
    pub upgrade_proven: bool,
}

impl From<HosExtensionV1> for HosExtension {
    fn from(old: HosExtensionV1) -> Self {
        Self {
            state_version: crate::STATE_VERSION,
            admins: old.admins,
            registry: old.registry,
            recovery: old.recovery,
            paused: old.paused,
            version: old.version,
            treasury: old.treasury,
            approved_code_hash: old.approved_code_hash,
            approved_at: old.approved_at,
            council: old.council,
            paused_until_ns: old.paused_until_ns,
            recovery_reset_pending: old.recovery_reset_pending,
            upgrade_proven: old.upgrade_proven,
            pending_council: None,
            pending_council_at: None,
        }
    }
}
