use near_sdk::serde::Serialize;
use near_sdk::FunctionError;

#[derive(Debug, Serialize)]
#[serde(crate = "near_sdk::serde", tag = "code", rename_all = "snake_case")]
pub enum ContractError {
    OnlyAdmin,
    OnlyCouncil,
    OnlyRegistry,
    Paused,
    CannotRemoveLastAdmin,
    InsufficientDeposit,
    InsufficientBalance,
    NotEd25519,
    EmptyCode,
    NoApprovedHash,
    HashMismatch,
    ApprovalTooYoung,
    ParkTakesNoOwner,
    TransferNeedsOwner,
    RequiresOneYocto,
}

impl FunctionError for ContractError {
    fn panic(&self) -> ! {
        hos_common::panic_json(self)
    }
}
