use crate::Registrar;
use near_sdk::{near, AccountId, NearToken};

#[near(serializers = [borsh])]
pub struct RegistrarV1 {
    pub state_version: u16,
    pub registry: AccountId,
    pub council: AccountId,
    pub wallet_impl: AccountId,
    pub hos_extension: AccountId,
    pub recovery: AccountId,
    pub chain_id: String,
    pub min_balance: NearToken,
    pub min_label_len: u8,
    pub wallet_timeout_secs: u32,
    pub approved_code_hash: Option<[u8; 32]>,
    pub approved_at: Option<u64>,
    pub config_epoch: u32,
    pub upgrade_proven: bool,
}

impl From<RegistrarV1> for Registrar {
    fn from(old: RegistrarV1) -> Self {
        Self {
            state_version: crate::STATE_VERSION,
            registry: old.registry,
            council: old.council,
            wallet_impl: old.wallet_impl,
            hos_extension: old.hos_extension,
            recovery: old.recovery,
            chain_id: old.chain_id,
            min_balance: old.min_balance,
            wallet_timeout_secs: old.wallet_timeout_secs,
            approved_code_hash: old.approved_code_hash,
            approved_at: old.approved_at,
            config_epoch: old.config_epoch,
            upgrade_proven: old.upgrade_proven,
            pending_council: None,
            pending_council_at: None,
        }
    }
}
