use near_sdk::{near, AccountId, NearToken};

#[near(event_json(standard = "hos_tla_registrar"))]
pub enum Event {
    #[event_version("1.0.0")]
    SubAccountMinted {
        account: AccountId,
        owner: AccountId,
    },
    #[event_version("1.0.0")]
    MintFailed { account: AccountId },
    #[event_version("1.0.0")]
    MinLabelLenSet { min_label_len: u8 },
    #[event_version("1.0.0")]
    MinBalanceSet { min_balance: NearToken },
    #[event_version("1.0.0")]
    SelfUpgraded {},
}
