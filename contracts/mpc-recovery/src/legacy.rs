use near_sdk::store::LookupMap;
use near_sdk::{near, AccountId, PublicKey};

use crate::state::{Account, ArmedPolicy};
use crate::MpcRecovery;

#[near(serializers = [borsh])]
pub struct MpcRecoveryV1 {
    pub state_version: u16,
    pub owner: AccountId,
    pub installer: AccountId,
    pub signer: AccountId,
    pub transfer_authority: AccountId,
    pub watchers: Vec<PublicKey>,
    pub threshold: u32,
    pub accounts: LookupMap<AccountId, Account>,
    pub round_floor: LookupMap<AccountId, u64>,
    pub approved_code_hash: Option<[u8; 32]>,
    pub approved_at: Option<u64>,
    pub registry: Option<AccountId>,
    pub armed: LookupMap<AccountId, ArmedPolicy>,
    pub upgrade_proven: bool,
}

impl From<MpcRecoveryV1> for MpcRecovery {
    fn from(old: MpcRecoveryV1) -> Self {
        Self {
            state_version: crate::STATE_VERSION,
            owner: old.owner,
            installer: old.installer,
            signer: old.signer,
            transfer_authority: old.transfer_authority,
            watchers: old.watchers,
            threshold: old.threshold,
            accounts: old.accounts,
            round_floor: old.round_floor,
            approved_code_hash: old.approved_code_hash,
            approved_at: old.approved_at,
            registry: old.registry,
            armed: old.armed,
            upgrade_proven: old.upgrade_proven,
            pending_owner: None,
            pending_owner_at: None,
        }
    }
}
