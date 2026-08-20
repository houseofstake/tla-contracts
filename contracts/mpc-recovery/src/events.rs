use near_sdk::json_types::U64;
use near_sdk::{near, AccountId, PublicKey};

#[near(event_json(standard = "hos_tla_recovery"))]
pub enum Event {
    #[event_version("1.0.0")]
    PolicyInstalled {
        account: AccountId,
        timelock_secs: u32,
        mpc_public_key: PublicKey,
        attestation_key: PublicKey,
    },
    #[event_version("1.0.0")]
    Requested {
        account: AccountId,
        round: U64,
        new_owner: PublicKey,
    },
    #[event_version("1.0.0")]
    Approved { account: AccountId, round: U64 },
    #[event_version("1.0.0")]
    Canceled { account: AccountId, round: U64 },
    #[event_version("1.0.0")]
    NativeSignatureProduced { account: AccountId, round: U64 },
    #[event_version("1.0.0")]
    Finalized { account: AccountId, round: U64 },
    #[event_version("1.0.0")]
    Aborted { account: AccountId, round: U64 },
    #[event_version("1.0.0")]
    InstallerChanged { installer: AccountId },
    #[event_version("1.0.0")]
    PolicyReset { account: AccountId },
    #[event_version("1.0.0")]
    PolicyResetDeferred { account: AccountId, round: U64 },
    #[event_version("1.0.0")]
    UpgradeApproved { hash: String, by: AccountId },
    #[event_version("1.0.0")]
    Upgraded {},
    #[event_version("1.0.0")]
    Sealed { public_key: String, by: AccountId },
    #[event_version("1.0.0")]
    WatchersChanged { threshold: u32, count: u32 },
    #[event_version("1.0.0")]
    RegistryChanged { registry: AccountId },
    #[event_version("1.0.0")]
    PolicyInstallArmed {
        account: AccountId,
        attestation_key: PublicKey,
        timelock_secs: u32,
    },
    #[event_version("1.0.0")]
    PolicyInstallDisarmed { account: AccountId },
    #[event_version("1.0.0")]
    NameRecoveryRequested {
        tla_id: AccountId,
        name: String,
        new_owner: AccountId,
        round: U64,
    },
    #[event_version("1.0.0")]
    NameRecoveryApproved {
        tla_id: AccountId,
        name: String,
        new_owner: AccountId,
    },
}
