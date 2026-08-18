use near_sdk::{near, AccountId};

#[near(event_json(standard = "hos_tla_impl_deployer"))]
pub enum Event {
    #[event_version("1.0.0")]
    HashApproved { hash: String, by: AccountId },
    #[event_version("1.0.0")]
    ImplDeployed { hash: String, size: u64 },
    #[event_version("1.0.0")]
    DeployFailed { hash: String },
    #[event_version("1.0.0")]
    SelfUpgraded {},
    #[event_version("1.0.0")]
    UpgradeApproved { hash: String, by: AccountId },
    #[event_version("1.0.0")]
    KeyDeleted { public_key: String, by: AccountId },
}
