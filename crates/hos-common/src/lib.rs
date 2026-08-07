use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, near, CurveType, PublicKey};

pub const FT_STORAGE_DEPOSIT_YOCTO: u128 = 1_250_000_000_000_000_000_000;

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
    ReRent,
    Reclaim,
    Recovery,
}

impl RotationCause {
    pub fn parks(self) -> bool {
        match self {
            Self::Reclaim => true,
            Self::Sale | Self::Transfer | Self::ReRent | Self::Recovery => false,
        }
    }

    pub fn sweeps(self) -> bool {
        match self {
            Self::Sale | Self::Transfer | Self::ReRent | Self::Reclaim => true,
            Self::Recovery => false,
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
