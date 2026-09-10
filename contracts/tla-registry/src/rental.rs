use crate::admin::MAX_ALLOWLIST_SIZE;
use crate::asset_gate::{
    ft_balance_fanout, ft_balances_clear, BalanceGate, FT_BALANCE_TGAS, GATE_CALLER_FRAME_TGAS,
};
use crate::callbacks::MintSettlement;
use crate::error::ContractError;
use crate::events::Event;
use crate::fees;
use crate::interfaces::{ext_hos_extension, ext_registrar};
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use hos_common::{OperatingState, RotationCause};
use near_sdk::json_types::{U128, U64};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, is_promise_success, near, AccountId, Gas, NearToken, Promise, PromiseOrValue};

pub(crate) const MAX_ORDER_ID_LEN: usize = 128;

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct PendingReRent {
    pub tla_id: AccountId,
    pub name: String,
    pub owner: AccountId,
    pub payer: AccountId,
    pub payout_account: AccountId,
    pub rent: U128,
    pub attached: U128,
    pub sub_account: AccountId,
    pub order_id: Option<String>,
}

const GAS_FOR_CREATE: Gas = Gas::from_tgas(hos_common::MINT_CALL_TGAS);
const GAS_FOR_SET_PAYOUT: Gas = Gas::from_tgas(30);
const GAS_FOR_SET_PAYOUT_CALLBACK: Gas = Gas::from_tgas(10);
const GAS_FOR_CALLBACK: Gas = Gas::from_tgas(15);
const GAS_FOR_RERENT_FORCE: Gas = Gas::from_tgas(45);
const GAS_FOR_PUSH_LEASE: Gas = Gas::from_tgas(20);
const GAS_FOR_RENEW_CALLBACK: Gas = Gas::from_tgas(10);
const LEASE_SYNC_CB_TGAS: u64 = 5;
const GAS_FOR_LEASE_SYNC_CB: Gas = Gas::from_tgas(LEASE_SYNC_CB_TGAS);
const RERENT_CALLBACK_TGAS: u64 = 40;
const GAS_FOR_RERENT_CALLBACK: Gas = Gas::from_tgas(RERENT_CALLBACK_TGAS);
const _: () = assert!(
    GAS_FOR_PUSH_LEASE.as_tgas() + LEASE_SYNC_CB_TGAS < RERENT_CALLBACK_TGAS,
    "the re-rent callback pushes the new term to the wallet out of its own budget, and a wallet left on the previous renter's expired lease refuses the buyer every method they paid for"
);
const RERENT_BALANCES_CB_TGAS: u64 = 85;
const GAS_FOR_RERENT_BALANCES_CB: Gas = Gas::from_tgas(RERENT_BALANCES_CB_TGAS);
const _: () = assert!(
    MAX_ALLOWLIST_SIZE as u64 * FT_BALANCE_TGAS + RERENT_BALANCES_CB_TGAS + GATE_CALLER_FRAME_TGAS
        <= 300,
    "the gate queries every allowlisted token before it dispatches, so widening the allowlist past what one call can fund breaks every re-rent"
);

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn activate_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        self.assert_not_paused()?;
        let caller = env::predecessor_account_id();
        let tla_len = tla_id.as_str().len() as u8;
        let rent_usd = fees::base_rent(tla_len, &self.fee_config);
        let required_usd = self
            .fee_config
            .tla_allocation_fee_usd_micro
            .0
            .saturating_add(rent_usd);
        let required_near = self.convert_usd_to_near(required_usd)?;
        let attached_yocto = env::attached_deposit().as_yoctonear();
        if attached_yocto < required_near {
            return Err(ContractError::InsufficientPayment);
        }

        let new_expires_at = {
            let entry = self
                .tlas
                .get_mut(&tla_id)
                .ok_or(ContractError::TlaNotFound)?;
            if entry.status != TlaStatus::Registered {
                return Err(ContractError::TlaNotInRegisteredState);
            }
            if entry.tla_type != TlaType::Business {
                return Err(ContractError::WrongActivationEndpoint);
            }
            let licensee = entry
                .licensee
                .as_ref()
                .ok_or(ContractError::BusinessTlaMissingLicensee)?;
            if &caller != licensee {
                return Err(ContractError::OnlyLicensee);
            }
            let now = env::block_timestamp();
            entry.status = TlaStatus::Active;
            entry.activated_at = now;
            entry.expires_at = now.saturating_add(ONE_YEAR_NS);
            entry.expires_at
        };

        self.total_revenue = self.total_revenue.saturating_add(required_near);
        self.refund_excess(&caller, attached_yocto, required_near);

        Event::TlaActivated {
            tla_id,
            expires_at: U64(new_expires_at),
            paid_yocto: U128(required_near),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn rent_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
        owner_account: Option<AccountId>,
    ) -> Result<Promise, ContractError> {
        self.assert_not_paused()?;
        validate_mintable_name(&tla_id, &name)?;

        let key = sub_account_key(&tla_id, &name);
        if self.sub_accounts.contains_key(&key) {
            return Err(ContractError::SubAccountNameTaken);
        }
        let is_re_rent = self.parked_names.contains_key(&key);
        if is_re_rent {
            self.assert_holds_nothing(&key)?;
        }

        let payer = env::predecessor_account_id();
        let owner = self.resolve_rent_owner(&payer, owner_account)?;
        let (rent, is_business) = self.quote_rent(&tla_id, &name, &owner)?;

        let rent_near = self.convert_usd_to_near(rent)?;
        let total = if is_re_rent {
            rent_near
        } else {
            rent_near.saturating_add(self.fee_config.account_creation_deposit_yocto.0)
        };
        let attached = env::attached_deposit();
        if attached.as_yoctonear() < total {
            return Err(ContractError::InsufficientPayment);
        }
        if is_business {
            self.business_count_check_and_bump(&tla_id)?;
        }

        let now = env::block_timestamp();
        let lease_until_ns = now.saturating_add(self.lease_term_ns);
        self.sub_account_insert(
            key.clone(),
            SubAccountEntry {
                owner: owner.clone(),
                tla_id: tla_id.clone(),
                payout_account: owner.clone(),
                rented_at: now,
                expires_at: lease_until_ns,
                retraction_at: None,
            },
        );

        let rent = U128(rent_near);
        let attached = U128(attached.as_yoctonear());
        if is_re_rent {
            let sub_account: AccountId = key
                .parse()
                .map_err(|_| ContractError::InvalidSubAccountId)?;
            return Ok(self.start_re_rent(PendingReRent {
                tla_id,
                name,
                payout_account: owner.clone(),
                owner,
                payer,
                rent,
                attached,
                sub_account,
                order_id: None,
            }));
        }

        let creation_deposit =
            NearToken::from_yoctonear(self.fee_config.account_creation_deposit_yocto.0);
        Ok(ext_registrar::ext(tla_id.clone())
            .with_attached_deposit(creation_deposit)
            .with_static_gas(GAS_FOR_CREATE)
            .create_sub_account(name.clone(), owner.clone(), owner.clone(), lease_until_ns)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_CALLBACK)
                    .on_sub_account_created(MintSettlement {
                        tla_id,
                        name,
                        owner,
                        payer,
                        rent_yocto: rent,
                        attached_yocto: attached,
                        order_id: None,
                    }),
            ))
    }

    #[handle_result]
    #[payable]
    pub fn rent_sub_account_paid(
        &mut self,
        tla_id: AccountId,
        name: String,
        owner_account: AccountId,
        payout_account: AccountId,
        order_id: String,
    ) -> Result<Promise, ContractError> {
        self.assert_not_paused()?;
        let payer = self.assert_payment_authority_for(&tla_id)?;
        validate_mintable_name(&tla_id, &name)?;
        self.assert_order_unsettled(&order_id)?;

        let key = sub_account_key(&tla_id, &name);
        if self.sub_accounts.contains_key(&key) {
            return Err(ContractError::SubAccountNameTaken);
        }
        let is_re_rent = self.parked_names.contains_key(&key);
        if payout_account.as_str() == key {
            return Err(ContractError::PayoutAccountEqualsSubAccount);
        }

        let is_business = self.assert_accepting(&tla_id, &payout_account)?;

        let creation_deposit = if is_re_rent {
            0
        } else {
            self.fee_config.account_creation_deposit_yocto.0
        };
        let attached = env::attached_deposit();
        if attached.as_yoctonear() < creation_deposit {
            return Err(ContractError::InsufficientPayment);
        }

        if is_business {
            self.business_count_check_and_bump(&tla_id)?;
        }

        let now = env::block_timestamp();
        let lease_until_ns = now.saturating_add(self.lease_term_ns);
        let sub_entry = SubAccountEntry {
            owner: owner_account.clone(),
            tla_id: tla_id.clone(),
            payout_account: payout_account.clone(),
            rented_at: now,
            expires_at: lease_until_ns,
            retraction_at: None,
        };
        self.sub_account_insert(key.clone(), sub_entry);
        self.record_authority_mint(&payer, &tla_id);
        self.paid_order_ids
            .insert(order_id.clone(), PaidOrderState::InFlight);
        Event::PaidRentalOrderReserved {
            full_name: key.clone(),
            order_id: order_id.clone(),
            by: payer.clone(),
        }
        .emit();

        if is_re_rent {
            let sub_account: AccountId = key
                .parse()
                .map_err(|_| ContractError::InvalidSubAccountId)?;
            return Ok(self.start_re_rent(PendingReRent {
                tla_id,
                name,
                owner: owner_account,
                payer,
                payout_account,
                rent: U128(0),
                attached: U128(attached.as_yoctonear()),
                sub_account,
                order_id: Some(order_id),
            }));
        }

        Ok(ext_registrar::ext(tla_id.clone())
            .with_attached_deposit(NearToken::from_yoctonear(creation_deposit))
            .with_static_gas(GAS_FOR_CREATE)
            .create_sub_account(
                name.clone(),
                owner_account.clone(),
                payout_account.clone(),
                lease_until_ns,
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_CALLBACK)
                    .on_sub_account_created_paid(MintSettlement {
                        tla_id,
                        name,
                        owner: owner_account,
                        payer,
                        rent_yocto: U128(0),
                        attached_yocto: U128(attached.as_yoctonear()),
                        order_id: Some(order_id),
                    }),
            ))
    }

    #[handle_result]
    #[payable]
    pub fn renew_tla(&mut self, tla_id: AccountId) -> Result<(), ContractError> {
        self.assert_not_paused()?;
        let caller = env::predecessor_account_id();
        let tla_len = tla_id.as_str().len() as u8;
        let rent_near = self.convert_usd_to_near(fees::base_rent(tla_len, &self.fee_config))?;
        let now = env::block_timestamp();

        let is_business;
        {
            let entry = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
            if entry.status != TlaStatus::Active {
                return Err(ContractError::TlaNotActive);
            }
            if now >= entry.expires_at.saturating_add(self.grace_period_ns) {
                return Err(ContractError::TlaPastGracePeriod);
            }
            is_business = entry.tla_type == TlaType::Business;
            if is_business {
                let licensee = entry
                    .licensee
                    .as_ref()
                    .ok_or(ContractError::BusinessTlaMissingLicensee)?;
                if &caller != licensee {
                    return Err(ContractError::OnlyLicensee);
                }
            }
        }
        if !is_business {
            self.assert_admin()?;
        }

        let attached = env::attached_deposit();
        if attached.as_yoctonear() < rent_near {
            return Err(ContractError::InsufficientPayment);
        }

        let new_expires_at = {
            let entry = self
                .tlas
                .get_mut(&tla_id)
                .ok_or(ContractError::TlaNotFound)?;
            let base = now.max(entry.expires_at);
            entry.expires_at = base.saturating_add(ONE_YEAR_NS);
            entry.expires_at
        };
        self.total_revenue = self.total_revenue.saturating_add(rent_near);
        self.refund_excess(&caller, attached.as_yoctonear(), rent_near);

        Event::TlaRenewed {
            tla_id,
            new_expires_at: U64(new_expires_at),
            paid_yocto: U128(rent_near),
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn set_payout_account(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_payout_account: AccountId,
    ) -> Result<Promise, ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        if new_payout_account.as_str() == key {
            return Err(ContractError::PayoutAccountEqualsSubAccount);
        }
        let caller = env::predecessor_account_id();
        let owner = {
            let sub = self
                .sub_accounts
                .get(&key)
                .ok_or(ContractError::SubAccountNotFound)?;
            let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
            if tla.tla_type == TlaType::Business {
                if tla.licensee.as_ref() != Some(&caller) {
                    return Err(ContractError::OnlyLicensee);
                }
            } else if caller != sub.owner {
                return Err(ContractError::OnlyOwner);
            }
            sub.owner.clone()
        };
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_SET_PAYOUT)
            .set_payout(sub_account, new_payout_account.clone(), owner.clone())
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_SET_PAYOUT_CALLBACK)
                    .on_payout_set(tla_id, name, new_payout_account, owner),
            ))
    }

    #[private]
    pub fn on_payout_set(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_payout_account: AccountId,
        expected_owner: AccountId,
    ) {
        let key = sub_account_key(&tla_id, &name);
        let still_theirs = self
            .sub_accounts
            .get(&key)
            .is_some_and(|sub| sub.owner == expected_owner);
        if !is_promise_success() || !still_theirs {
            Event::PayoutAccountUpdateFailed {
                full_name: key,
                attempted: new_payout_account,
            }
            .emit();
            return;
        }
        if let Some(sub) = self.sub_accounts.get_mut(&key) {
            sub.payout_account = new_payout_account.clone();
        }
        Event::PayoutAccountUpdated {
            full_name: key,
            new_payout_account,
        }
        .emit();
    }

    #[handle_result]
    #[payable]
    pub fn renew_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<Promise, ContractError> {
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        let caller = env::predecessor_account_id();
        let now = env::block_timestamp();

        let rent;
        {
            let sub = self
                .sub_accounts
                .get(&key)
                .ok_or(ContractError::SubAccountNotFound)?;
            if now >= sub.expires_at.saturating_add(self.grace_period_ns) {
                return Err(ContractError::SubAccountPastGracePeriod);
            }
            if sub.retraction_at.is_some() {
                return Err(ContractError::RetractionPending);
            }
            let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
            if matches!(
                tla.lifecycle(&self.clock(), self.suspension_expiry(&tla_id)),
                LifecycleStatus::Reclaimable
            ) {
                return Err(ContractError::TlaPastGracePeriod);
            }
            rent = fees::calculate_rent(tla, &tla_id, &name, &self.fee_config);
        }

        let rent_near = self.convert_usd_to_near(rent)?;
        let attached = env::attached_deposit();
        if attached.as_yoctonear() < rent_near {
            return Err(ContractError::InsufficientPayment);
        }

        let lease_term_ns = self.lease_term_ns;
        let new_expires_at = {
            let sub = self
                .sub_accounts
                .get(&key)
                .ok_or(ContractError::SubAccountNotFound)?;
            now.max(sub.expires_at).saturating_add(lease_term_ns)
        };
        self.refund_excess(&caller, attached.as_yoctonear(), rent_near);

        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_PUSH_LEASE)
            .push_lease(sub_account, U64(new_expires_at), OperatingState::Active)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_RENEW_CALLBACK)
                    .on_sub_account_renewed(
                        tla_id,
                        name,
                        U64(new_expires_at),
                        caller,
                        U128(rent_near),
                    ),
            ))
    }

    #[private]
    pub fn on_lease_synced(&mut self, sub_account: String, intent: String) {
        if is_promise_success() {
            return;
        }
        Event::LeaseSyncFailed {
            full_name: sub_account,
            intent,
        }
        .emit();
    }

    #[private]
    pub fn on_sub_account_renewed(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_expires_at: U64,
        payer: AccountId,
        rent_yocto: U128,
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !is_promise_success() {
            self.add_pending_refund(&payer, rent_yocto.0);
            Event::SubAccountRenewFailed {
                full_name: key,
                refunded_yocto: rent_yocto,
            }
            .emit();
            return;
        }
        let Some(sub) = self.sub_accounts.get_mut(&key) else {
            self.add_pending_refund(&payer, rent_yocto.0);
            Event::SubAccountRenewFailed {
                full_name: key,
                refunded_yocto: rent_yocto,
            }
            .emit();
            return;
        };
        sub.expires_at = new_expires_at.0;
        self.total_revenue = self.total_revenue.saturating_add(rent_yocto.0);
        self.emit_activity(Event::SubAccountRenewed {
            full_name: key,
            new_expires_at,
            paid_yocto: rent_yocto,
        });
    }

    #[private]
    pub fn on_re_rent_balances_checked(
        &mut self,
        pending: PendingReRent,
        allowlist: Vec<AccountId>,
    ) -> PromiseOrValue<()> {
        let key = sub_account_key(&pending.tla_id, &pending.name);
        if let BalanceGate::Blocked { token, reason } = ft_balances_clear(&allowlist) {
            self.settle_failed_mint(
                &key,
                &pending.tla_id,
                &pending.payer,
                pending.attached,
                "re-rent blocked by asset gate",
            );
            Event::SubAccountSaleBlocked {
                full_name: key,
                token,
                reason,
            }
            .emit();
            return PromiseOrValue::Value(());
        }
        PromiseOrValue::Promise(re_rent_transfer(&self.hos_extension, pending))
    }
}

impl TlaRegistry {
    pub(crate) fn assert_holds_nothing(&self, key: &str) -> Result<(), ContractError> {
        let account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        if self.owner_supply(&account) > 0 {
            return Err(ContractError::NameStillHoldsNames);
        }
        Ok(())
    }

    fn resolve_rent_owner(
        &self,
        payer: &AccountId,
        owner_account: Option<AccountId>,
    ) -> Result<AccountId, ContractError> {
        match owner_account {
            None => Ok(payer.clone()),
            Some(owner) if &owner == payer => Ok(owner),
            Some(owner) => {
                self.assert_payment_authority()?;
                Ok(owner)
            }
        }
    }

    fn assert_accepting(
        &self,
        tla_id: &AccountId,
        licensee_must_be: &AccountId,
    ) -> Result<bool, ContractError> {
        let suspended_until = self.suspension_expiry(tla_id);
        let entry = self.tlas.get(tla_id).ok_or(ContractError::TlaNotFound)?;
        if !entry.accepting_rentals(suspended_until) {
            return Err(ContractError::TlaNotAcceptingRentals);
        }
        let is_business = entry.tla_type == TlaType::Business;
        if is_business {
            let licensee = entry
                .licensee
                .as_ref()
                .ok_or(ContractError::BusinessTlaMissingLicensee)?;
            if licensee_must_be != licensee {
                return Err(ContractError::OnlyLicensee);
            }
        }
        Ok(is_business)
    }

    fn assert_order_unsettled(&self, order_id: &str) -> Result<(), ContractError> {
        if order_id.is_empty() || order_id.len() > MAX_ORDER_ID_LEN {
            return Err(ContractError::InvalidOrderId);
        }
        if self.paid_order_ids.contains_key(order_id) {
            return Err(ContractError::PaidOrderAlreadySettled);
        }
        Ok(())
    }

    fn quote_rent(
        &self,
        tla_id: &AccountId,
        name: &str,
        owner: &AccountId,
    ) -> Result<(u128, bool), ContractError> {
        let is_business = self.assert_accepting(tla_id, owner)?;
        let entry = self.tlas.get(tla_id).ok_or(ContractError::TlaNotFound)?;
        Ok((
            fees::calculate_rent(entry, tla_id, name, &self.fee_config),
            is_business,
        ))
    }

    fn start_re_rent(&self, pending: PendingReRent) -> Promise {
        let allowlist: Vec<AccountId> = self.ft_allowlist.iter().cloned().collect();
        let Some(chain) = ft_balance_fanout(&allowlist, &pending.sub_account) else {
            return re_rent_transfer(&self.hos_extension, pending);
        };
        chain.then(
            Self::ext(env::current_account_id())
                .with_static_gas(GAS_FOR_RERENT_BALANCES_CB)
                .on_re_rent_balances_checked(pending, allowlist),
        )
    }
}

fn watch_lease_sync(pushed: Promise, sub_account: &AccountId, intent: &str) -> Promise {
    pushed.then(
        TlaRegistry::ext(env::current_account_id())
            .with_static_gas(GAS_FOR_LEASE_SYNC_CB)
            .on_lease_synced(sub_account.to_string(), intent.to_string()),
    )
}

pub(crate) fn retract_wallet_lease(
    hos_extension: &AccountId,
    sub_account: AccountId,
    ends_at: u64,
) -> Promise {
    let pushed = ext_hos_extension::ext(hos_extension.clone())
        .with_static_gas(GAS_FOR_PUSH_LEASE)
        .retract_lease(sub_account.clone(), U64(ends_at));
    watch_lease_sync(pushed, &sub_account, "retract")
}

pub(crate) fn push_re_rented_lease(
    hos_extension: &AccountId,
    sub_account: AccountId,
    expires_at: u64,
) -> Promise {
    let pushed = ext_hos_extension::ext(hos_extension.clone())
        .with_static_gas(GAS_FOR_PUSH_LEASE)
        .push_lease(sub_account.clone(), U64(expires_at), OperatingState::Active);
    watch_lease_sync(pushed, &sub_account, "re-rent")
}

fn re_rent_transfer(hos_extension: &AccountId, pending: PendingReRent) -> Promise {
    ext_hos_extension::ext(hos_extension.clone())
        .with_static_gas(GAS_FOR_RERENT_FORCE)
        .force_transfer(
            pending.sub_account,
            Some(pending.owner.clone()),
            RotationCause::ReRent,
            None,
        )
        .then(
            TlaRegistry::ext(env::current_account_id())
                .with_static_gas(GAS_FOR_RERENT_CALLBACK)
                .on_sub_account_re_rented(MintSettlement {
                    tla_id: pending.tla_id,
                    name: pending.name,
                    owner: pending.owner,
                    payer: pending.payer,
                    rent_yocto: pending.rent,
                    attached_yocto: pending.attached,
                    order_id: pending.order_id,
                }),
        )
}
