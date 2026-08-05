use near_sdk::serde::Serialize;
use near_sdk::FunctionError;

#[derive(Debug, Serialize)]
#[serde(crate = "near_sdk::serde", tag = "code", rename_all = "snake_case")]
pub enum ContractError {
    OnlyAdmin,
    OnlyCouncil,
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
    SignerNotPending,
    PayoutAccountEqualsSubAccount,
    InvalidSubAccountId,
    InvalidName { reason: NameInvalidReason },
    InsufficientPayment,
    InsufficientRevenue,
    WithdrawalAmountZero,
    TokenNotInAllowlist,
    AllowlistFull,
    AllRentTiersZero,
    CreationDepositZero,
    CannotRemoveLastAdmin,
    MaxBusinessSubsReached,
    NoRetractionScheduled,
    RetractionAlreadyScheduled,
    RetractionAlreadyElapsed,
    RetractionPending,
    NotBusinessTla,
    RequiresOneYocto,
    InsufficientContractBalance,
    NotListed,
    SaleInProgress,
    ReclaimInProgress,
    SubAccountTlaMismatch,
    SubAccountNotSellable,
    BusinessSubNotResellable,
    PriceNotMet,
    InvalidPrice,
    NoAcceptedOffer,
    InvalidCommissionRate,
    NotEd25519,
    SameOwner,
    SameOwnerKey,
    TransferToSubAccount,
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
