use hos_common::{OperatingState, RotationCause};
use near_sdk::json_types::{U128, U64};
use near_sdk::{ext_contract, AccountId};

#[allow(dead_code)]
#[ext_contract(ext_hos_extension)]
pub trait HosExtension {
    fn sweep_ft(&mut self, wallet: AccountId, ft: AccountId);
    fn sweep_near(&mut self, wallet: AccountId);
    fn force_transfer(
        &mut self,
        wallet: AccountId,
        new_owner: Option<AccountId>,
        cause: RotationCause,
    );
    fn push_lease(&mut self, wallet: AccountId, lease_until_ns: U64, state: OperatingState);
}

#[allow(dead_code)]
#[ext_contract(ext_ft)]
pub trait FungibleToken {
    fn ft_balance_of(&self, account_id: AccountId) -> U128;
}

#[allow(dead_code)]
#[ext_contract(ext_registrar)]
pub trait Registrar {
    fn create_sub_account(
        &mut self,
        name: String,
        owner_account: AccountId,
        payout_account: AccountId,
        lease_until_ns: u64,
    );
}
