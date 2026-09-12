use crate::error::ContractError;
use crate::events::Event;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use hos_common::MintOutcome;
use near_sdk::json_types::{U128, U64};
use near_sdk::{env, near, AccountId, FunctionError, PromiseError, PromiseOrValue};

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
    pub order_id: Option<String>,
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
        let order = settlement.order_id.clone();
        match outcome {
            Ok(MintOutcome::Active) => {
                self.confirm_order(order.as_ref(), &key, &settlement.payer);
                let expires_at = self.record_rental(
                    &key,
                    &settlement.payer,
                    settlement.rent_yocto,
                    settlement.attached_yocto,
                );
                crate::nft::emit_nft_mint(&settlement.owner, &key);
                self.emit_activity(Event::SubAccountRented {
                    full_name: key,
                    tla_id: settlement.tla_id,
                    owner: settlement.owner,
                    rent_yocto: settlement.rent_yocto,
                    expires_at: U64(expires_at),
                });
            }
            Ok(MintOutcome::CreationFailed) => {
                self.release_order(order.as_ref(), &key, &settlement.payer, &settlement.tla_id);
                self.settle_failed_mint(
                    &key,
                    &settlement.tla_id,
                    &settlement.payer,
                    settlement.attached_yocto,
                    "sub-account creation failed",
                );
            }
            Err(_) => {
                self.release_order(order.as_ref(), &key, &settlement.payer, &settlement.tla_id);
                self.settle_stranded_mint(
                    &key,
                    &settlement.tla_id,
                    &settlement.payer,
                    settlement.attached_yocto,
                    "sub-account mint chain failed without an outcome",
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
        let order = settlement.order_id.clone();
        match outcome {
            Ok(MintOutcome::Active) => {
                self.confirm_order(order.as_ref(), &key, &settlement.payer);
                let expires_at =
                    self.record_paid_rental(&key, &settlement.payer, settlement.attached_yocto);
                crate::nft::emit_nft_mint(&settlement.owner, &key);
                self.emit_activity(Event::SubAccountRented {
                    full_name: key,
                    tla_id: settlement.tla_id,
                    owner: settlement.owner,
                    rent_yocto: settlement.rent_yocto,
                    expires_at: U64(expires_at),
                });
            }
            Ok(MintOutcome::CreationFailed) => {
                self.release_order(order.as_ref(), &key, &settlement.payer, &settlement.tla_id);
                self.settle_failed_mint(
                    &key,
                    &settlement.tla_id,
                    &settlement.payer,
                    settlement.attached_yocto,
                    "paid sub-account creation failed",
                );
            }
            Err(_) => {
                self.release_order(order.as_ref(), &key, &settlement.payer, &settlement.tla_id);
                self.settle_stranded_mint(
                    &key,
                    &settlement.tla_id,
                    &settlement.payer,
                    settlement.attached_yocto,
                    "paid sub-account mint chain failed without an outcome",
                );
            }
        }
    }

    #[private]
    pub fn on_sub_account_re_rented(
        &mut self,
        settlement: MintSettlement,
        #[callback_result] swapped: Result<bool, PromiseError>,
    ) -> PromiseOrValue<()> {
        let key = sub_account_key(&settlement.tla_id, &settlement.name);
        let order = settlement.order_id.clone();
        if !matches!(swapped, Ok(true)) {
            self.release_order(order.as_ref(), &key, &settlement.payer, &settlement.tla_id);
            self.settle_failed_mint(
                &key,
                &settlement.tla_id,
                &settlement.payer,
                settlement.attached_yocto,
                "sub-account re-rent failed",
            );
            return PromiseOrValue::Value(());
        }
        self.confirm_order(order.as_ref(), &key, &settlement.payer);
        self.parked_names.remove(&key);
        crate::nft::emit_nft_mint(&settlement.owner, &key);
        self.sub_account_count = self.sub_account_count.saturating_add(1);
        self.total_revenue = self.total_revenue.saturating_add(settlement.rent_yocto.0);
        self.refund_excess(
            &settlement.payer,
            settlement.attached_yocto.0,
            settlement.rent_yocto.0,
        );
        let expires_at = match self.sub_accounts.get(&key) {
            Some(s) => s.expires_at,
            None => ContractError::SubAccountNotFound.panic(),
        };
        self.emit_activity(Event::SubAccountReRented {
            full_name: key.clone(),
            tla_id: settlement.tla_id,
            owner: settlement.owner,
            rent_yocto: settlement.rent_yocto,
            expires_at: U64(expires_at),
        });
        PromiseOrValue::Value(())
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

    pub(crate) fn release_order(
        &mut self,
        order_id: Option<&String>,
        full_name: &str,
        payer: &AccountId,
        tla_id: &AccountId,
    ) {
        let Some(order_id) = order_id else {
            return;
        };
        let still_ours = self
            .paid_order_ids
            .get(order_id)
            .is_some_and(|reserved| reserved.in_flight_for(full_name, payer));
        if !still_ours {
            Event::PaidRentalOrderStale {
                order_id: order_id.clone(),
                full_name: full_name.to_string(),
            }
            .emit();
            return;
        }
        self.paid_order_ids.remove(order_id);
        self.release_authority_mint(payer, tla_id);
        Event::PaidRentalOrderReleased {
            order_id: order_id.clone(),
        }
        .emit();
    }

    pub(crate) fn confirm_order(
        &mut self,
        order_id: Option<&String>,
        full_name: &str,
        payer: &AccountId,
    ) {
        let Some(order_id) = order_id else {
            return;
        };
        let Some(reserved) = self.paid_order_ids.get(order_id) else {
            Event::PaidRentalOrderStale {
                order_id: order_id.clone(),
                full_name: full_name.to_string(),
            }
            .emit();
            return;
        };
        if !reserved.in_flight_for(full_name, payer) {
            Event::PaidRentalOrderStale {
                order_id: order_id.clone(),
                full_name: full_name.to_string(),
            }
            .emit();
            return;
        }
        self.paid_order_ids
            .insert(order_id.clone(), PaidOrderState::Settled);
        Event::PaidRentalOrderSettled {
            full_name: full_name.to_string(),
            order_id: order_id.clone(),
        }
        .emit();
    }

    pub(crate) fn settle_failed_mint(
        &mut self,
        key: &str,
        tla_id: &AccountId,
        payer: &AccountId,
        attached: U128,
        reason: &str,
    ) {
        if self.sub_account_remove(key).is_none() {
            return;
        }
        self.business_count_decrement_if_business(tla_id);
        self.add_pending_refund(payer, attached.0);
        Event::RefundPending {
            account: payer.clone(),
            amount_yocto: attached,
            reason: reason.to_string(),
        }
        .emit();
    }

    pub(crate) fn settle_stranded_mint(
        &mut self,
        key: &str,
        tla_id: &AccountId,
        payer: &AccountId,
        attached: U128,
        reason: &str,
    ) {
        if self.sub_account_remove(key).is_none() {
            return;
        }
        self.business_count_decrement_if_business(tla_id);
        self.add_pending_refund(payer, attached.0);
        self.parked_names.insert(
            key.to_string(),
            ParkedEntry {
                tla_id: tla_id.clone(),
                parked_at: env::block_timestamp(),
            },
        );
        Event::RefundPending {
            account: payer.clone(),
            amount_yocto: attached,
            reason: reason.to_string(),
        }
        .emit();
        Event::MintFundingStranded {
            full_name: key.to_string(),
            tla_id: tla_id.clone(),
            payer: payer.clone(),
            account_creation_deposit_yocto: self.fee_config.account_creation_deposit_yocto,
            reason: reason.to_string(),
        }
        .emit();
    }
}
