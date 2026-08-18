use near_sdk::serde::Serialize;
use near_sdk::FunctionError;

#[derive(Debug, Serialize)]
#[serde(crate = "near_sdk::serde", tag = "code", rename_all = "snake_case")]
pub enum ContractError {
    OnlyAdmin,
    OnlyCouncil,
    MarketplacePaused,
    OnlyPaymentAuthority,
    OnlyRecoveryAuthority,
    OnlyPriceOracle,
    RateOutOfBounds,
    RateNotInitialized,
    RateAlreadyInitialized,
    RateCooldown,
    RateStale,
    InvalidRateBounds,
    InvalidBusinessCap,
    RetractionNoticeTooShort,
    FeeExceedsCap,
    OnlyLicensee,
    OnlyOwner,
    Paused,
    NoPendingRefund,
    TlaNotFound,
    TlaAlreadyRegistered,
    TlaNotInRegisteredState,
    TlaNotActive,
    TlaNotSuspended,
    TlaNotAcceptingRentals,
    TlaPastGracePeriod,
    BusinessTlaRequiresLicensee,
    BusinessTlaMissingLicensee,
    WrongActivationEndpoint,
    SubAccountNotFound,
    SubAccountNameTaken,
    SubAccountPastGracePeriod,
    SubAccountNotReclaimable,
    PayoutAccountEqualsSubAccount,
    InvalidSubAccountId,
    InvalidName { reason: NameInvalidReason },
    InsufficientPayment,
    InsufficientRevenue,
    WithdrawalAmountZero,
    TokenNotInAllowlist,
    SubAccountHoldsTokens,
    AllowlistFull,
    VenueIsRegistry,
    AllRentTiersZero,
    RentTiersNotDescending,
    CreationDepositZero,
    CannotRemoveLastAdmin,
    MaxBusinessSubsReached,
    NoRetractionScheduled,
    RetractionAlreadyScheduled,
    RetractionAlreadyElapsed,
    RetractionPending,
    NotBusinessTla,
    RequiresOneYocto,
    EmptyCode,
    NoApprovedHash,
    HashMismatch,
    ApprovalTooYoung,
    InsufficientContractBalance,
    ReclaimInProgress,
    SubAccountTlaMismatch,
    SubAccountNotSellable,
    BusinessSubNotResellable,
    ApprovalsNotSupported,
    TokenNotFound,
    RotationNotConfirmed,
    OwnerIndexOutOfSync,
    NotEd25519,
    SameOwner,
    TransferToSubAccount,
    TransferToRegisteredName,
    OwnerMoved,
}

#[derive(Debug, Serialize)]
#[serde(crate = "near_sdk::serde", rename_all = "snake_case")]
pub enum NameInvalidReason {
    LengthOutOfBounds,
    DisallowedCharacter,
    EdgeSeparator,
}

impl FunctionError for ContractError {
    fn panic(&self) -> ! {
        hos_common::panic_json(self)
    }
}
