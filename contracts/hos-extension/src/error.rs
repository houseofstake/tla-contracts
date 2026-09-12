use near_sdk::serde::Serialize;
use near_sdk::FunctionError;

#[derive(Debug, Serialize)]
#[serde(crate = "near_sdk::serde", tag = "code", rename_all = "snake_case")]
pub enum ContractError {
    OnlyAdmin,
    OnlyCouncil,
    OnlyRegistry,
    NoPendingReset,
    NoPendingSweep,
    Paused,
    CannotRemoveLastAdmin,
    InsufficientDeposit,
    InsufficientBalance,
    AdminSetFull,
    NotEd25519,
    EmptyCode,
    NoApprovedHash,
    HashMismatch,
    ApprovalTooYoung,
    ParkTakesNoOwner,
    ReRentNeedsOwnCall,
    TransferNeedsOwner,
    TransferNeedsHolder,
    RequiresOneYocto,
    UpgradeNotProven,
    CouncilUnchanged,
    CouncilIsSelf,
    NoCouncilRotationPending,
    OnlyPendingCouncil,
    CouncilRotationTooYoung,
}

impl FunctionError for ContractError {
    fn panic(&self) -> ! {
        hos_common::panic_json(self)
    }
}
pub const STATE_VERSION_UNKNOWN: &str = "state version is not the one this code understands";
pub const NO_STATE: &str = "no contract state to migrate";
