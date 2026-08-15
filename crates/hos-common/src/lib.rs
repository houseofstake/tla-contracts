use near_sdk::borsh::BorshDeserialize;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, near, CurveType, Gas, NearToken, Promise, PublicKey};

pub const FT_STORAGE_DEPOSIT_YOCTO: u128 = 1_250_000_000_000_000_000_000;

pub const MAX_AUTHORITY_HOLD_NS: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;

const STATE_KEY: &[u8] = b"STATE";

const GAS_FOR_DEPLOY_AND_MIGRATE: Gas = Gas::from_tgas(60);

pub fn try_state_read<T: BorshDeserialize>() -> Option<T> {
    env::storage_read(STATE_KEY).and_then(|raw| T::try_from_slice(&raw).ok())
}

pub fn deploy_and_migrate(code: Vec<u8>) -> Promise {
    Promise::new(env::current_account_id())
        .deploy_contract(code)
        .function_call(
            "migrate".to_owned(),
            Vec::new(),
            NearToken::from_near(0),
            env::prepaid_gas()
                .saturating_sub(env::used_gas())
                .saturating_sub(GAS_FOR_DEPLOY_AND_MIGRATE),
        )
}

#[near(serializers = [borsh, json])]
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum OperatingState {
    Active,
    Listed,
    Settling,
    Suspended,
    Parked,
}

pub fn is_ed25519(key: &PublicKey) -> bool {
    key.curve_type() == CurveType::ED25519
}

pub fn panic_json<T: Serialize>(err: &T) -> ! {
    let json = near_sdk::serde_json::to_string(err)
        .unwrap_or_else(|_| String::from(r#"{"code":"serialization_failure"}"#));
    env::panic_str(&json)
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(crate = "near_sdk::serde")]
pub enum MintOutcome {
    CreationFailed,
    Active,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(crate = "near_sdk::serde")]
pub enum RotationCause {
    Sale,
    Transfer,
    Deposit,
    ReRent,
    Reclaim,
    Recovery,
    /// Undoes the rotation immediately preceding it, and nothing else. The
    /// wallet pins the destination to the account the name just left and
    /// refuses anything older or elsewhere.
    Revert,
}

impl RotationCause {
    pub fn parks(self) -> bool {
        match self {
            Self::Reclaim => true,
            Self::Sale
            | Self::Transfer
            | Self::Deposit
            | Self::ReRent
            | Self::Recovery
            | Self::Revert => false,
        }
    }

    /// A venue holds a name without ever having a beneficial claim on what sits
    /// on the account, so payout must stay with the depositor. Repointing it at
    /// the venue sends the next sweep somewhere no one can recover it from.
    pub fn repoints_payout(self) -> bool {
        match self {
            Self::Deposit => false,
            Self::Sale
            | Self::Transfer
            | Self::ReRent
            | Self::Reclaim
            | Self::Recovery
            | Self::Revert => true,
        }
    }

    /// A revert must not sweep. The rotation it undoes already swept the
    /// balance to the outgoing payout account, and sweeping again would empty
    /// the account on its way back to the owner who never let go of it.
    pub fn sweeps(self) -> bool {
        match self {
            Self::Sale | Self::Transfer | Self::Deposit | Self::ReRent | Self::Reclaim => true,
            Self::Recovery | Self::Revert => false,
        }
    }

    pub fn needs_holder(self) -> bool {
        match self {
            Self::Sale | Self::Transfer | Self::Deposit => true,
            Self::ReRent | Self::Reclaim | Self::Recovery | Self::Revert => false,
        }
    }

    pub fn needs_expiry(self) -> bool {
        match self {
            Self::ReRent | Self::Reclaim => true,
            Self::Sale | Self::Transfer | Self::Deposit | Self::Recovery | Self::Revert => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn accepts_ed25519() {
        let key =
            PublicKey::from_str("ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847").unwrap();
        assert!(is_ed25519(&key));
    }

    #[test]
    fn rejects_non_ed25519() {
        let secp = PublicKey::from_str(
            "secp256k1:qMoRgcoXai4mBPsdbHi1wfyxF9TdbPCF4qSDQTRP3TfescSRoUdSx6nmeQoN3aiwGzwMyGXAb1gUjBTv5AY8DXj",
        )
        .unwrap();
        assert!(!is_ed25519(&secp));
    }
}

#[cfg(test)]
mod state_reading {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::testing_env;

    #[near(serializers = [borsh])]
    #[derive(PartialEq, Debug)]
    struct Old {
        a: u64,
    }

    #[near(serializers = [borsh])]
    #[derive(PartialEq, Debug)]
    struct New {
        a: u64,
        b: u64,
    }

    fn ctx() {
        testing_env!(VMContextBuilder::new().build());
    }

    #[test]
    fn state_key_matches_the_sdk() {
        ctx();
        env::state_write(&Old { a: 7 });
        assert_eq!(
            try_state_read::<Old>(),
            Some(Old { a: 7 }),
            "near-sdk moved STATE_KEY out from under try_state_read"
        );
    }

    #[test]
    fn a_shape_that_does_not_match_reads_as_none_instead_of_panicking() {
        ctx();
        env::state_write(&Old { a: 7 });
        assert_eq!(
            try_state_read::<New>(),
            None,
            "this is the migration case: it must be answerable, not fatal"
        );
    }

    #[test]
    fn trailing_bytes_are_refused_so_a_longer_state_cannot_pose_as_a_shorter_one() {
        ctx();
        env::state_write(&New { a: 7, b: 9 });
        assert_eq!(try_state_read::<Old>(), None);
    }

    #[test]
    fn an_empty_account_reads_as_none() {
        ctx();
        assert_eq!(try_state_read::<Old>(), None);
    }
}
