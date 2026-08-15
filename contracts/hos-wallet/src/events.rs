use near_sdk::json_types::{U128, U64};
use near_sdk::{near, AccountId};

use crate::state::OperatingState;

#[near(event_json(standard = "hos_tla_tenant_wallet"))]
pub enum Event {
    #[event_version("1.0.0")]
    WalletInitialized { owner: AccountId },
    #[event_version("1.0.0")]
    LeaseSet {
        until_ns: U64,
        state: OperatingState,
    },
    #[event_version("1.0.0")]
    PayoutAccountSet { payout_account: AccountId },
    #[event_version("1.0.0")]
    SpendGranted { extension: AccountId },
    #[event_version("1.0.0")]
    SpendRevoked { extension: AccountId },
    #[event_version("1.0.0")]
    SweptNear {
        payout_account: AccountId,
        amount: U128,
    },
    #[event_version("1.0.0")]
    SweptFt {
        payout_account: AccountId,
        ft: AccountId,
        amount: U128,
    },
    #[event_version("1.0.0")]
    Frozen { self_initiated: bool },
    #[event_version("1.0.0")]
    Unfrozen {},
}
