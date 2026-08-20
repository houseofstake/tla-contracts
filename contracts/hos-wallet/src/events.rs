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
    ImplPinned { code_hash: String },
    #[event_version("1.0.0")]
    ImplApproved { code_hash: String },
    #[event_version("1.0.0")]
    ImplCancelled {},
    #[event_version("1.0.0")]
    SpendGranted { extension: AccountId },
    #[event_version("1.0.0")]
    SpendRevoked { extension: AccountId },
    #[event_version("1.0.0")]
    SpendCharged {
        extension: AccountId,
        token: Option<AccountId>,
        receiver: AccountId,
        amount: U128,
        spent: U128,
    },
    #[event_version("1.0.0")]
    ItemSpent {
        extension: AccountId,
        collection: AccountId,
        token_id: String,
        receiver: AccountId,
    },
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
    SweepFailed {
        payout_account: AccountId,
        ft: Option<AccountId>,
        amount: U128,
    },
    #[event_version("1.0.0")]
    Frozen { self_initiated: bool },
    #[event_version("1.0.0")]
    Unfrozen {},
}
