use near_sdk::near;

pub use hos_common::OperatingState;

#[near(serializers = [borsh, json])]
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum FreezeState {
    Unfrozen,
    SelfFrozen,
    AuthorityFrozen,
}
