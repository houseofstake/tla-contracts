use near_sdk::json_types::{U128, U64};
use near_sdk::{near, AccountId};

use crate::types::{PremiumCategory, TlaType};

#[near(event_json(standard = "hos_tla_registry"))]
pub enum Event {
    #[event_version("1.0.0")]
    NearUsdRateUpdated {
        previous_micro: U128,
        new_micro: U128,
        sequence: U64,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    PriceOracleUpdated { account: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    RateRecoveredFromOutOfBand {
        stranded_micro: U128,
        new_micro: U128,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    TlaRegistered {
        tla_id: AccountId,
        tla_type: TlaType,
        premium_category: PremiumCategory,
        licensee: Option<AccountId>,
    },
    #[event_version("1.0.0")]
    TlaActivated {
        tla_id: AccountId,
        expires_at: U64,
        paid_yocto: U128,
    },
    #[event_version("1.0.0")]
    TlaSuspended { tla_id: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    TlaUnsuspended { tla_id: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    TlaRenewed {
        tla_id: AccountId,
        new_expires_at: U64,
        paid_yocto: U128,
    },
    #[event_version("1.0.0")]
    ContractPaused { by: AccountId },
    #[event_version("1.0.0")]
    ContractUnpaused { by: AccountId },
    #[event_version("1.0.0")]
    MarketplacePaused { by: AccountId },
    #[event_version("1.0.0")]
    MarketplaceUnpaused { by: AccountId },
    #[event_version("1.0.0")]
    FeeConfigUpdated { by: AccountId },
    #[event_version("1.0.0")]
    AdminAdded { account: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    AdminRemoved { account: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    PaymentAuthorityAdded { account: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    PaymentAuthorityRemoved { account: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    RecoveryAuthorityAdded { account: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    RecoveryAuthorityRemoved { account: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    SubAccountRecovered {
        full_name: String,
        tla_id: AccountId,
        from: AccountId,
        to: AccountId,
    },
    #[event_version("1.0.0")]
    WithdrawalQueued {
        amount_yocto: U128,
        recipient: AccountId,
    },
    #[event_version("1.0.0")]
    FtAllowlistAdded { token: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    FtAllowlistRemoved { token: AccountId, by: AccountId },
    #[event_version("1.0.0")]
    BusinessSubCapSet {
        tla_id: AccountId,
        cap: Option<u32>,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    SubAccountRented {
        full_name: String,
        tla_id: AccountId,
        owner: AccountId,
        rent_yocto: U128,
        expires_at: U64,
    },
    #[event_version("1.0.0")]
    SubAccountReRented {
        full_name: String,
        tla_id: AccountId,
        owner: AccountId,
        rent_yocto: U128,
        expires_at: U64,
    },
    #[event_version("1.0.0")]
    SubAccountRenewed {
        full_name: String,
        new_expires_at: U64,
        paid_yocto: U128,
    },
    #[event_version("1.0.0")]
    PayoutAccountUpdated {
        full_name: String,
        new_payout_account: AccountId,
    },
    #[event_version("1.0.0")]
    RefundPending {
        account: AccountId,
        amount_yocto: U128,
        reason: String,
    },
    #[event_version("1.0.0")]
    SubAccountTransferred {
        full_name: String,
        tla_id: AccountId,
        from: AccountId,
        to: AccountId,
    },
    #[event_version("1.0.0")]
    TransferFailed {
        full_name: String,
        from: AccountId,
        to: AccountId,
    },
    #[event_version("1.0.0")]
    SubAccountSaleBlocked {
        full_name: String,
        token: Option<AccountId>,
        reason: String,
    },
    #[event_version("1.0.0")]
    SubAccountReclaimed {
        full_name: String,
        tla_id: AccountId,
        swept_to: AccountId,
    },
    #[event_version("1.0.0")]
    ReclaimFinalizeBlocked {
        full_name: String,
        token: Option<AccountId>,
        reason: String,
    },
    #[event_version("1.0.0")]
    SubAccountRetractionScheduled {
        full_name: String,
        retraction_at: U64,
        by: AccountId,
    },
    #[event_version("1.0.0")]
    SubAccountRetractionCanceled { full_name: String, by: AccountId },
    #[event_version("1.0.0")]
    ReclaimPendingCleared { full_name: String, by: AccountId },
    #[event_version("1.0.0")]
    UpgradeApproved { hash: String, by: AccountId },
    #[event_version("1.0.0")]
    Upgraded { by: AccountId },
}

impl Event {
    pub(crate) fn activity_entry(&self) -> Option<(&'static str, String)> {
        match self {
            Self::SubAccountRented { full_name, .. } => {
                Some(("sub_account_rented", full_name.clone()))
            }
            Self::SubAccountReRented { full_name, .. } => {
                Some(("sub_account_re_rented", full_name.clone()))
            }
            Self::SubAccountRenewed { full_name, .. } => {
                Some(("sub_account_renewed", full_name.clone()))
            }
            Self::SubAccountTransferred { full_name, .. } => {
                Some(("sub_account_transferred", full_name.clone()))
            }
            Self::SubAccountRecovered { full_name, .. } => {
                Some(("sub_account_recovered", full_name.clone()))
            }
            Self::SubAccountReclaimed { full_name, .. } => {
                Some(("sub_account_reclaimed", full_name.clone()))
            }
            _ => None,
        }
    }
}
