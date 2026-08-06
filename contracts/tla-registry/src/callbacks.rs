use crate::error::ContractError;
use crate::events::Event;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use hos_common::MintOutcome;
use near_sdk::json_types::{U128, U64};
use near_sdk::{is_promise_success, near, AccountId, FunctionError, PromiseError};

/// `owner` receives the lease, `payer` receives refunds. A sponsored mint
/// sets them to different accounts.
#[near(serializers = [json])]
pub struct MintSettlement {
    pub tla_id: AccountId,
    pub name: String,
    pub owner: AccountId,
    pub payer: AccountId,
    pub rent_yocto: U128,
    pub attached_yocto: U128,
}

#[near]
impl TlaRegistry {
    #[private]
    pub fn on_sub_account_created(
        &mut self,
        settlement: MintSettlement,
        #[callback_result] outcome: Result<MintOutcome, PromiseError>,
    ) {
        let key = sub_account_key(&settlement.tla_id, &settlement.name);
        match outcome {
            Ok(MintOutcome::Active) => {
                let expires_at = self.record_rental(
                    &key,
                    &settlement.payer,
                    settlement.rent_yocto,
                    settlement.attached_yocto,
                );
                self.emit_activity(Event::SubAccountRented {
                    full_name: key,
                    tla_id: settlement.tla_id,
                    owner: settlement.owner,
                    rent_yocto: settlement.rent_yocto,
                    expires_at: U64(expires_at),
                });
            }
            Ok(MintOutcome::CreationFailed) | Err(_) => {
                self.settle_failed_mint(
                    &key,
                    &settlement.tla_id,
                    &settlement.payer,
                    settlement.attached_yocto,
                    "sub-account creation failed",
                );
            }
        }
    }

    #[private]
    pub fn on_sub_account_created_paid(
        &mut self,
        settlement: MintSettlement,
        #[callback_result] outcome: Result<MintOutcome, PromiseError>,
    ) {
        let key = sub_account_key(&settlement.tla_id, &settlement.name);
        match outcome {
            Ok(MintOutcome::Active) => {
                let expires_at =
                    self.record_paid_rental(&key, &settlement.payer, settlement.attached_yocto);
                self.emit_activity(Event::SubAccountRented {
                    full_name: key,
                    tla_id: settlement.tla_id,
                    owner: settlement.owner,
                    rent_yocto: settlement.rent_yocto,
                    expires_at: U64(expires_at),
                });
            }
            Ok(MintOutcome::CreationFailed) | Err(_) => {
                self.settle_failed_mint(
                    &key,
                    &settlement.tla_id,
                    &settlement.payer,
                    settlement.attached_yocto,
                    "paid sub-account creation failed",
                );
            }
        }
    }

    #[private]
    pub fn on_sub_account_re_rented(&mut self, settlement: MintSettlement) {
        let key = sub_account_key(&settlement.tla_id, &settlement.name);
        if !is_promise_success() {
            self.settle_failed_mint(
                &key,
                &settlement.tla_id,
                &settlement.payer,
                settlement.attached_yocto,
                "sub-account re-rent failed",
            );
            return;
        }
        self.parked_names.remove(&key);
        self.sub_account_count = self.sub_account_count.saturating_add(1);
        self.total_revenue = self.total_revenue.saturating_add(settlement.rent_yocto.0);
        self.refund_excess(
            &settlement.payer,
            settlement.attached_yocto.0,
            settlement.rent_yocto.0,
        );
        let Some(expires_at) = self.sub_accounts.get(&key).map(|s| s.expires_at) else {
            return;
        };
        self.emit_activity(Event::SubAccountReRented {
            full_name: key,
            tla_id: settlement.tla_id,
            owner: settlement.owner,
            rent_yocto: settlement.rent_yocto,
            expires_at: U64(expires_at),
        });
    }
}

impl TlaRegistry {
    fn record_rental(
        &mut self,
        key: &str,
        payer: &AccountId,
        rent_yocto: U128,
        attached_yocto: U128,
    ) -> u64 {
        self.sub_account_count = self.sub_account_count.saturating_add(1);
        self.total_revenue = self.total_revenue.saturating_add(rent_yocto.0);
        let charged = rent_yocto
            .0
            .saturating_add(self.fee_config.account_creation_deposit_yocto.0);
        self.refund_excess(payer, attached_yocto.0, charged);
        match self.sub_accounts.get(key) {
            Some(s) => s.expires_at,
            None => ContractError::SubAccountNotFound.panic(),
        }
    }

    fn record_paid_rental(&mut self, key: &str, payer: &AccountId, attached_yocto: U128) -> u64 {
        self.sub_account_count = self.sub_account_count.saturating_add(1);
        self.refund_excess(
            payer,
            attached_yocto.0,
            self.fee_config.account_creation_deposit_yocto.0,
        );
        match self.sub_accounts.get(key) {
            Some(s) => s.expires_at,
            None => ContractError::SubAccountNotFound.panic(),
        }
    }

    pub(crate) fn settle_failed_mint(
        &mut self,
        key: &str,
        tla_id: &AccountId,
        payer: &AccountId,
        attached: U128,
        reason: &str,
    ) {
        self.sub_account_remove(key);
        self.business_count_decrement_if_business(tla_id);
        self.add_pending_refund(payer, attached.0);
        Event::RefundPending {
            account: payer.clone(),
            amount_yocto: attached,
            reason: reason.to_string(),
        }
        .emit();
    }
}
