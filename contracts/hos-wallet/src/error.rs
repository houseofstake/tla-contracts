pub const ONLY_PARENT: &str = "init caller must be the direct parent account";
pub const ONLY_AUTHORITY: &str = "only the lease authority may perform this operation";
pub const ONLY_RENTER: &str = "only the renter may perform this operation";
pub const ONLY_OWNER: &str = "only the owner may perform this operation";
pub const NO_STATE: &str = "no state to migrate";
pub const UNAUTHORIZED: &str = "unauthorized";
pub const AUTHORITY_PROTECTED: &str = "the lease authority extension cannot be removed";
pub const AUTHORITY_IS_SELF: &str = "authority must not be this account";
pub const SELF_TARGET: &str = "receiver must not be this account";
pub const NOT_ACTIVE: &str = "wallet is not active";
pub const LEASE_EXPIRED: &str = "lease expired";
pub const FROZEN: &str = "wallet frozen";
pub const NOT_FROZEN: &str = "wallet not frozen";
pub const SELF_FROZEN: &str = "wallet is self-frozen and only the renter may unfreeze it";
pub const AUTHORITY_FROZEN: &str =
    "wallet is authority-frozen and only the authority may unfreeze it";
pub const INVALID_TIMEOUT: &str = "timeout_secs out of bounds";
pub const LEASE_IN_PAST: &str = "lease_until_ns must be in the future";
pub const LEASE_NOT_MONOTONIC: &str = "lease end may not move backwards";
pub const BAD_LEASE_STATE: &str = "state not settable through lease push";
pub const RESERVE_BREACH: &str = "action would breach the balance reserve";
pub const DEPOSIT_OVERFLOW: &str = "action deposits overflow";
pub const PAYOUT_IS_SELF: &str = "payout account must not be this account";
pub const OWNER_MOVED: &str = "the account changed hands before this payout change landed";
pub const OWNER_IS_SELF: &str = "owner account must not be this account";
pub const COLLECTION_IS_SELF: &str = "collection account must not be this account";
pub const LEASE_ACTIVE: &str = "sweep requires an expired lease";
pub const EXTENSIONS_LOCKED: &str = "the authority may edit extensions only after the lease ends";
pub const TRANSFER_NOT_REQUESTED: &str =
    "a transfer of a live lease must be asked for by an account that holds it";
pub const NOTHING_TO_REVERT: &str = "there is no rotation to revert";
pub const REVERT_WINDOW_CLOSED: &str = "the revert window for this rotation has closed";
pub const REVERT_TARGET_PINNED: &str = "a revert may only return the name to where it came from";
pub const NOTHING_TO_SWEEP: &str = "nothing to sweep";
pub const NO_SPEND_GRANT: &str = "this extension has no spend grant";
pub const GRANT_EXPIRED: &str = "spend grant expired";
pub const RECEIVER_NOT_GRANTED: &str = "receiver is not in the spend grant";
pub const GRANT_ACTION_NOT_ALLOWED: &str =
    "a spend grant covers plain transfers and allowlisted token calls only, never deploying code";
pub const GRANT_METHOD_NOT_ALLOWED: &str =
    "a spend grant covers ft_transfer and nft_transfer only, because nothing else states what it moves";
pub const GRANT_CALL_MUST_STAND_ALONE: &str = "a granted token call may carry no other action";
pub const GRANT_CALL_DEPOSIT: &str = "a granted token call attaches exactly one yocto";
pub const GRANT_ARGS_UNREADABLE: &str = "granted token call arguments are not readable";
pub const GRANT_APPROVAL_NOT_ALLOWED: &str = "a granted transfer cannot spend an approval";
pub const TOKEN_NOT_GRANTED: &str = "token is not in the spend grant";
pub const TOKEN_CAP_EXCEEDED: &str = "spend exceeds the granted cap for this token";
pub const TOKEN_LISTED_TWICE: &str = "a token may carry only one budget";
pub const COLLECTION_NOT_GRANTED: &str = "collection is not in the spend grant";
pub const COLLECTION_LISTED_TWICE: &str = "a collection may carry only one item list";
pub const ITEM_NOT_GRANTED: &str = "item is not in the spend grant";
pub const EMPTY_ITEM_GRANT: &str = "an item grant needs at least one token id";
/// A name is an account, so its contents cannot be known when the grant is
/// written. Nothing else in the grant can bound it.
pub const OWN_COLLECTION_NOT_GRANTABLE: &str = "a spend grant cannot move this account's own names";
pub const REFUND_TARGET_NOT_ALLOWED: &str = "a granted spend cannot redirect refunds";
pub const GRANT_CAP_EXCEEDED: &str = "spend exceeds the granted cap";
pub const EMPTY_GRANT: &str = "a spend grant needs at least one receiver";
pub const GRANT_IN_PAST: &str = "grant expiry must be in the future";
pub const AUTHORIZATION_NOT_JSON: &str = "authorization is not valid json";
pub const AUTHORITY_NOT_OWNER: &str = "the lease authority cannot authorise as an owner";
pub const NOT_AN_OWNER: &str = "named account is not an owner of this account";
pub const GRANTEE_NOT_OWNER: &str = "a spend grantee cannot authorise as an owner";
