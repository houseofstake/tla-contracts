use near_sdk::{near, AccountId, PublicKey};

#[near(serializers = [borsh])]
#[derive(Clone)]
pub struct ArmedPolicy {
    pub attestation_key: PublicKey,
    pub timelock_secs: u32,
}

#[near(serializers = [borsh])]
#[derive(Clone)]
pub struct Policy {
    pub mpc_public_key: PublicKey,
    pub attestation_key: PublicKey,
    pub timelock_secs: u32,
}

#[near(serializers = [borsh])]
pub enum Phase {
    Idle,
    Requested {
        new_owner: PublicKey,
        round: u64,
        requested_at: u64,
    },
    Approved {
        new_owner: PublicKey,
        round: u64,
    },
    Resolving {
        new_owner: PublicKey,
        round: u64,
    },
    NameRequested {
        new_owner: AccountId,
        round: u64,
        requested_at: u64,
    },
    NameResolving {
        new_owner: AccountId,
        round: u64,
        requested_at: u64,
    },
}

impl Phase {
    pub fn pending(&self) -> Option<(&PublicKey, u64)> {
        match self {
            Phase::Requested {
                new_owner, round, ..
            }
            | Phase::Approved { new_owner, round }
            | Phase::Resolving { new_owner, round } => Some((new_owner, *round)),
            Phase::Idle | Phase::NameRequested { .. } | Phase::NameResolving { .. } => None,
        }
    }

    pub fn name_resolving(&self, at_round: u64) -> Option<(AccountId, u64)> {
        match self {
            Phase::NameResolving {
                new_owner,
                round,
                requested_at,
            } if *round == at_round => Some((new_owner.clone(), *requested_at)),
            _ => None,
        }
    }

    pub fn name_pending(&self) -> Option<(&AccountId, u64, u64)> {
        match self {
            Phase::NameRequested {
                new_owner,
                round,
                requested_at,
            } => Some((new_owner, *round, *requested_at)),
            Phase::Idle
            | Phase::Requested { .. }
            | Phase::Approved { .. }
            | Phase::Resolving { .. }
            | Phase::NameResolving { .. } => None,
        }
    }

    pub fn resolving_owner(&self, round: u64) -> Option<PublicKey> {
        match self {
            Phase::Resolving {
                new_owner,
                round: resolving_round,
            } if *resolving_round == round => Some(new_owner.clone()),
            _ => None,
        }
    }
}

#[near(serializers = [borsh])]
pub struct Account {
    pub policy: Policy,
    pub round: u64,
    pub phase: Phase,
}
