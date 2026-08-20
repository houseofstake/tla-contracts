pub const ONLY_COUNCIL: &str = "only council";
pub const NO_APPROVED_HASH: &str = "no approved code hash";
pub const HASH_MISMATCH: &str = "code does not match the approved hash";
pub const DEPLOY_IN_FLIGHT: &str = "another deploy is in flight";
pub const APPROVAL_TOO_YOUNG: &str = "approved code must wait out the delay before publishing";
pub const INSUFFICIENT_DEPOSIT: &str = "attached deposit below global storage cost";
pub const EMPTY_CODE: &str = "code must not be empty";
pub const COST_OVERFLOW: &str = "storage cost overflow";
pub const REQUIRES_ONE_YOCTO: &str = "requires an attached deposit of exactly 1 yoctoNEAR";
pub const COUNCIL_IS_SELF: &str = "council must not be this account, which ends with no keys";
pub const NO_APPROVED_UPGRADE: &str = "no approved upgrade hash";
pub const UPGRADE_HASH_MISMATCH: &str = "code does not match the approved upgrade hash";
pub const UPGRADE_TOO_YOUNG: &str =
    "an approved upgrade must wait out the delay before it installs";
pub const NO_STATE: &str = "no state to migrate";
pub const STATE_VERSION_UNKNOWN: &str = "state version is not the one this code understands";
pub const UPGRADE_NOT_PROVEN: &str = "the upgrade path has not been exercised on this account yet";
