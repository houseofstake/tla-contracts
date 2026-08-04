use crate::asset_gate::{ft_balance_fanout, ft_balances_clear, BalanceGate};
use crate::error::ContractError;
use crate::events::Event;
use crate::fees;
use crate::interfaces::ext_hos_extension;
use crate::lifecycle::effective_sub_lifecycle;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::{env, near, AccountId, Gas, Promise, PromiseOrValue};

const GAS_FOR_FORCE_TRANSFER: Gas = Gas::from_tgas(45);
const GAS_FOR_SOLD_CALLBACK: Gas = Gas::from_tgas(20);
const GAS_FOR_BUY_BALANCES_CB: Gas = Gas::from_tgas(85);
const GAS_FOR_TRANSFER_CALLBACK: Gas = Gas::from_tgas(20);

#[derive(Serialize, Deserialize)]
#[serde(crate = "near_sdk::serde")]
pub struct PendingBuy {
    pub tla_id: AccountId,
    pub name: String,
    pub buyer: AccountId,
    pub price: U128,
    pub deposit: U128,
    pub sub_account: AccountId,
}

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn list_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
        price: U128,
    ) -> Result<(), ContractError> {
        let seller = self.assert_listable(&tla_id, &name, price)?;
        let key = sub_account_key(&tla_id, &name);
        self.listings.insert(
            key.clone(),
            Listing {
                price: price.0,
                settling: false,
                seller: seller.clone(),
            },
        );
        Event::SubAccountListed {
            full_name: key,
            price_yocto: price,
            seller,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn transfer_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_owner: AccountId,
    ) -> Result<Promise, ContractError> {
        let (sub_account, from) = self.assert_transferable(&tla_id, &name, &new_owner)?;
        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_FORCE_TRANSFER)
            .force_transfer(sub_account, Some(new_owner.clone()), false)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_TRANSFER_CALLBACK)
                    .on_sub_account_transferred(tla_id, name, from, new_owner),
            ))
    }

    #[handle_result]
    #[payable]
    pub fn recover_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_owner: AccountId,
    ) -> Result<Promise, ContractError> {
        crate::assert_one_yocto()?;
        self.assert_recovery_authority()?;
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        if self.reclaim_pending.contains_key(&key) {
            return Err(ContractError::ReclaimInProgress);
        }
        self.assert_sale_idle(&key)?;
        let from = self
            .sub_accounts
            .get(&key)
            .ok_or(ContractError::SubAccountNotFound)?
            .owner
            .clone();
        if new_owner == from {
            return Err(ContractError::SameOwner);
        }
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        if new_owner == sub_account {
            return Err(ContractError::TransferToSubAccount);
        }
        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_FORCE_TRANSFER)
            .force_transfer(sub_account, Some(new_owner.clone()), false)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_TRANSFER_CALLBACK)
                    .on_sub_account_recovered(tla_id, name, from, new_owner),
            ))
    }

    #[private]
    pub fn on_sub_account_recovered(
        &mut self,
        tla_id: AccountId,
        name: String,
        from: AccountId,
        to: AccountId,
        #[callback_result] swapped: Result<bool, near_sdk::PromiseError>,
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !matches!(swapped, Ok(true)) {
            Event::TransferFailed {
                full_name: key,
                from,
                to,
            }
            .emit();
            return;
        }
        let Some(sub) = self.sub_accounts.get_mut(&key) else {
            Event::TransferFailed {
                full_name: key,
                from,
                to,
            }
            .emit();
            return;
        };
        sub.owner = to.clone();
        sub.payout_account = to.clone();
        self.listings.remove(&key);
        self.accepted_offers.remove(&key);
        Event::SubAccountRecovered {
            full_name: key,
            tla_id,
            from,
            to,
        }
        .emit();
    }

    #[private]
    pub fn on_sub_account_transferred(
        &mut self,
        tla_id: AccountId,
        name: String,
        from: AccountId,
        to: AccountId,
        #[callback_result] swapped: Result<bool, near_sdk::PromiseError>,
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !matches!(swapped, Ok(true)) {
            Event::TransferFailed {
                full_name: key,
                from,
                to,
            }
            .emit();
            return;
        }
        match self.sub_accounts.get_mut(&key) {
            Some(sub) => {
                sub.owner = to.clone();
                sub.payout_account = to.clone();
            }
            None => {
                Event::TransferFailed {
                    full_name: key,
                    from,
                    to,
                }
                .emit();
                return;
            }
        }
        self.listings.remove(&key);
        self.accepted_offers.remove(&key);
        Event::SubAccountTransferred {
            full_name: key,
            tla_id,
            from,
            to,
        }
        .emit();
    }

    #[handle_result]
    #[payable]
    pub fn unlist_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        if !self.listings.contains_key(&key) {
            return Err(ContractError::NotListed);
        }
        let owner = self.sub_account_owner(&key)?;
        if env::predecessor_account_id() != owner {
            return Err(ContractError::OnlyOwner);
        }
        self.listings.remove(&key);
        Event::SubAccountUnlisted {
            full_name: key,
            by: owner,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn accept_offer(
        &mut self,
        tla_id: AccountId,
        name: String,
        buyer: AccountId,
        price: U128,
    ) -> Result<(), ContractError> {
        let seller = self.assert_listable(&tla_id, &name, price)?;
        let key = sub_account_key(&tla_id, &name);
        self.accepted_offers.insert(
            key.clone(),
            AcceptedOffer {
                buyer: buyer.clone(),
                price: price.0,
                settling: false,
                seller: seller.clone(),
            },
        );
        Event::OfferAccepted {
            full_name: key,
            buyer,
            price_yocto: price,
            seller,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn revoke_offer(&mut self, tla_id: AccountId, name: String) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        if !self.accepted_offers.contains_key(&key) {
            return Err(ContractError::NoAcceptedOffer);
        }
        let owner = self.sub_account_owner(&key)?;
        if env::predecessor_account_id() != owner {
            return Err(ContractError::OnlyOwner);
        }
        self.accepted_offers.remove(&key);
        Event::OfferRevoked {
            full_name: key,
            by: owner,
        }
        .emit();
        Ok(())
    }

    #[handle_result]
    #[payable]
    pub fn buy_sub_account(
        &mut self,
        tla_id: AccountId,
        name: String,
    ) -> Result<Promise, ContractError> {
        self.assert_not_paused()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        self.assert_sellable(&key, &tla_id)?;
        let buyer = env::predecessor_account_id();
        let deposit = env::attached_deposit().as_yoctonear();
        let price = self.resolve_and_lock_sale(&key, &buyer, deposit)?;
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        let settlement = PendingBuy {
            tla_id,
            name,
            buyer,
            price: U128(price),
            deposit: U128(deposit),
            sub_account,
        };
        let allowlist: Vec<AccountId> = self.ft_allowlist.iter().cloned().collect();
        let Some(chain) = ft_balance_fanout(&allowlist, &settlement.sub_account) else {
            return Ok(settle_transfer(&self.hos_extension, settlement));
        };
        Ok(chain.then(
            Self::ext(env::current_account_id())
                .with_static_gas(GAS_FOR_BUY_BALANCES_CB)
                .on_buy_balances_checked(settlement, allowlist),
        ))
    }

    #[handle_result]
    pub fn buy_sub_account_paid(
        &mut self,
        tla_id: AccountId,
        name: String,
        buyer: AccountId,
    ) -> Result<PromiseOrValue<()>, ContractError> {
        self.assert_not_paused()?;
        let operator = self.assert_payment_authority()?;
        validate_name(&name)?;
        let key = sub_account_key(&tla_id, &name);
        self.assert_sale_idle(&key)?;
        self.assert_sellable(&key, &tla_id)?;
        let price = self.resolve_and_lock_paid_sale(&key, &buyer)?;
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        let settlement = PendingBuy {
            tla_id,
            name,
            buyer,
            price: U128(price),
            deposit: U128(0),
            sub_account,
        };
        let allowlist: Vec<AccountId> = self.ft_allowlist.iter().cloned().collect();
        let Some(chain) = ft_balance_fanout(&allowlist, &settlement.sub_account) else {
            return Ok(PromiseOrValue::Promise(settle_transfer_paid(
                &self.hos_extension,
                settlement,
                operator,
            )));
        };
        Ok(PromiseOrValue::Promise(
            chain.then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_BUY_BALANCES_CB)
                    .on_buy_paid_balances_checked(settlement, allowlist, operator),
            ),
        ))
    }

    #[private]
    pub fn on_sub_account_sold(
        &mut self,
        tla_id: AccountId,
        name: String,
        buyer: AccountId,
        price: U128,
        deposit: U128,
        #[callback_result] swapped: Result<bool, near_sdk::PromiseError>,
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !matches!(swapped, Ok(true)) {
            self.add_pending_refund(&buyer, deposit.0);
            self.clear_settling(&key);
            Event::SubAccountSaleFailed {
                full_name: key,
                buyer,
            }
            .emit();
            return;
        }
        let seller = match self
            .sub_accounts
            .get(&key)
            .map(|s| s.payout_account.clone())
        {
            Some(payout_account) => payout_account,
            None => {
                self.add_pending_refund(&buyer, deposit.0);
                self.clear_settling(&key);
                return;
            }
        };
        let (commission, seller_proceeds) =
            fees::split_resale(price.0, self.fee_config.resale_commission_bps);
        self.total_revenue = self.total_revenue.saturating_add(commission);
        self.add_pending_refund(&seller, seller_proceeds);
        self.refund_excess(&buyer, deposit.0, price.0);
        if let Some(sub) = self.sub_accounts.get_mut(&key) {
            sub.owner = buyer.clone();
            sub.payout_account = buyer.clone();
        }
        self.listings.remove(&key);
        self.accepted_offers.remove(&key);
        Event::SubAccountSold {
            full_name: key,
            tla_id,
            seller,
            buyer,
            price_yocto: price,
            commission_yocto: U128(commission),
            seller_proceeds_yocto: U128(seller_proceeds),
        }
        .emit();
    }

    #[private]
    pub fn on_sub_account_sold_paid(
        &mut self,
        tla_id: AccountId,
        name: String,
        buyer: AccountId,
        operator: AccountId,
        price: U128,
        #[callback_result] swapped: Result<bool, near_sdk::PromiseError>,
    ) {
        let key = sub_account_key(&tla_id, &name);
        if !matches!(swapped, Ok(true)) {
            self.clear_settling(&key);
            Event::SubAccountSaleFailed {
                full_name: key,
                buyer,
            }
            .emit();
            return;
        }
        let seller = match self
            .sub_accounts
            .get(&key)
            .map(|s| s.payout_account.clone())
        {
            Some(owner) => owner,
            None => {
                self.clear_settling(&key);
                return;
            }
        };
        if let Some(sub) = self.sub_accounts.get_mut(&key) {
            sub.owner = operator;
            sub.payout_account = buyer.clone();
        }
        self.listings.remove(&key);
        self.accepted_offers.remove(&key);
        Event::SubAccountSold {
            full_name: key,
            tla_id,
            seller,
            buyer,
            price_yocto: price,
            commission_yocto: U128(0),
            seller_proceeds_yocto: U128(0),
        }
        .emit();
    }

    #[private]
    pub fn on_buy_balances_checked(
        &mut self,
        settlement: PendingBuy,
        allowlist: Vec<AccountId>,
    ) -> PromiseOrValue<()> {
        let key = sub_account_key(&settlement.tla_id, &settlement.name);
        if let BalanceGate::Blocked { token, reason } = ft_balances_clear(&allowlist) {
            return self.abort_sale(&key, &settlement, token, &reason);
        }
        PromiseOrValue::Promise(settle_transfer(&self.hos_extension, settlement))
    }

    #[private]
    pub fn on_buy_paid_balances_checked(
        &mut self,
        settlement: PendingBuy,
        allowlist: Vec<AccountId>,
        operator: AccountId,
    ) -> PromiseOrValue<()> {
        let key = sub_account_key(&settlement.tla_id, &settlement.name);
        if let BalanceGate::Blocked { token, reason } = ft_balances_clear(&allowlist) {
            self.clear_settling(&key);
            Event::SubAccountSaleBlocked {
                full_name: key,
                token,
                reason,
            }
            .emit();
            return PromiseOrValue::Value(());
        }
        PromiseOrValue::Promise(settle_transfer_paid(
            &self.hos_extension,
            settlement,
            operator,
        ))
    }

    pub fn get_listing(&self, tla_id: AccountId, name: String) -> Option<ListingView> {
        let key = sub_account_key(&tla_id, &name);
        self.listings.get(&key).map(|l| ListingView {
            full_name: key.clone(),
            price_yocto: U128(l.price),
            settling: l.settling,
        })
    }

    pub fn get_accepted_offer(&self, tla_id: AccountId, name: String) -> Option<AcceptedOfferView> {
        let key = sub_account_key(&tla_id, &name);
        self.accepted_offers.get(&key).map(|o| AcceptedOfferView {
            full_name: key.clone(),
            buyer: o.buyer.clone(),
            price_yocto: U128(o.price),
            settling: o.settling,
        })
    }
}

impl TlaRegistry {
    fn assert_listable(
        &self,
        tla_id: &AccountId,
        name: &str,
        price: U128,
    ) -> Result<AccountId, ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        validate_name(name)?;
        if price.0 == 0 {
            return Err(ContractError::InvalidPrice);
        }
        let key = sub_account_key(tla_id, name);
        self.assert_sale_idle(&key)?;
        let owner = self.assert_sellable(&key, tla_id)?;
        if env::predecessor_account_id() != owner {
            return Err(ContractError::OnlyOwner);
        }
        Ok(owner)
    }

    fn assert_transferable(
        &self,
        tla_id: &AccountId,
        name: &str,
        new_owner: &AccountId,
    ) -> Result<(AccountId, AccountId), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_not_paused()?;
        validate_name(name)?;
        let key = sub_account_key(tla_id, name);
        if self.reclaim_pending.contains_key(&key) {
            return Err(ContractError::ReclaimInProgress);
        }
        self.assert_sale_idle(&key)?;
        let owner = self.assert_sellable(&key, tla_id)?;
        if env::predecessor_account_id() != owner {
            return Err(ContractError::OnlyOwner);
        }
        if *new_owner == owner {
            return Err(ContractError::SameOwner);
        }
        let sub_account: AccountId = key
            .parse()
            .map_err(|_| ContractError::InvalidSubAccountId)?;
        if *new_owner == sub_account {
            return Err(ContractError::TransferToSubAccount);
        }
        Ok((sub_account, owner))
    }

    pub(crate) fn assert_sale_idle(&self, key: &str) -> Result<(), ContractError> {
        if let Some(listing) = self.listings.get(key) {
            if listing.settling {
                return Err(ContractError::SaleInProgress);
            }
        }
        if let Some(offer) = self.accepted_offers.get(key) {
            if offer.settling {
                return Err(ContractError::SaleInProgress);
            }
        }
        Ok(())
    }

    fn assert_sellable(&self, key: &str, tla_id: &AccountId) -> Result<AccountId, ContractError> {
        if self.reclaim_pending.contains_key(key) {
            return Err(ContractError::ReclaimInProgress);
        }
        let sub = self
            .sub_accounts
            .get(key)
            .ok_or(ContractError::SubAccountNotFound)?;
        if sub.tla_id != *tla_id {
            return Err(ContractError::SubAccountTlaMismatch);
        }
        if sub.retraction_at.is_some() {
            return Err(ContractError::RetractionPending);
        }
        let tla = self.tlas.get(tla_id).ok_or(ContractError::TlaNotFound)?;
        if tla.tla_type == TlaType::Business {
            return Err(ContractError::BusinessSubNotResellable);
        }
        if tla.status != TlaStatus::Active {
            return Err(ContractError::SubAccountNotSellable);
        }
        if !matches!(
            effective_sub_lifecycle(
                sub,
                tla,
                self.fee_config.retraction_notice_ns.0,
                self.grace_period_ns,
            ),
            LifecycleStatus::Active
        ) {
            return Err(ContractError::SubAccountNotSellable);
        }
        Ok(sub.owner.clone())
    }

    fn sub_account_owner(&self, key: &str) -> Result<AccountId, ContractError> {
        self.sub_accounts
            .get(key)
            .map(|s| s.owner.clone())
            .ok_or(ContractError::SubAccountNotFound)
    }

    fn resolve_and_lock_sale(
        &mut self,
        key: &str,
        buyer: &AccountId,
        deposit: u128,
    ) -> Result<u128, ContractError> {
        let offer_terms = self
            .accepted_offers
            .get(key)
            .map(|o| (o.buyer.clone(), o.price));
        if let Some((offer_buyer, offer_price)) = offer_terms {
            if &offer_buyer == buyer {
                if deposit < offer_price {
                    return Err(ContractError::PriceNotMet);
                }
                if let Some(offer) = self.accepted_offers.get_mut(key) {
                    offer.settling = true;
                }
                return Ok(offer_price);
            }
        }
        let listing_price = self.listings.get(key).map(|l| l.price);
        if let Some(listing_price) = listing_price {
            if deposit < listing_price {
                return Err(ContractError::PriceNotMet);
            }
            if let Some(listing) = self.listings.get_mut(key) {
                listing.settling = true;
            }
            return Ok(listing_price);
        }
        Err(ContractError::NotListed)
    }

    fn resolve_and_lock_paid_sale(
        &mut self,
        key: &str,
        buyer: &AccountId,
    ) -> Result<u128, ContractError> {
        let offer_terms = self
            .accepted_offers
            .get(key)
            .map(|o| (o.buyer.clone(), o.price));
        if let Some((offer_buyer, offer_price)) = offer_terms {
            if &offer_buyer == buyer {
                if let Some(offer) = self.accepted_offers.get_mut(key) {
                    offer.settling = true;
                }
                return Ok(offer_price);
            }
        }
        let listing_price = self.listings.get(key).map(|l| l.price);
        if let Some(listing_price) = listing_price {
            if let Some(listing) = self.listings.get_mut(key) {
                listing.settling = true;
            }
            return Ok(listing_price);
        }
        Err(ContractError::NotListed)
    }

    pub(crate) fn clear_settling(&mut self, key: &str) {
        if let Some(listing) = self.listings.get_mut(key) {
            listing.settling = false;
        }
        if let Some(offer) = self.accepted_offers.get_mut(key) {
            offer.settling = false;
        }
    }

    fn abort_sale(
        &mut self,
        key: &str,
        settlement: &PendingBuy,
        token: Option<AccountId>,
        reason: &str,
    ) -> PromiseOrValue<()> {
        self.add_pending_refund(&settlement.buyer, settlement.deposit.0);
        self.clear_settling(key);
        Event::SubAccountSaleBlocked {
            full_name: key.to_string(),
            token,
            reason: reason.to_string(),
        }
        .emit();
        PromiseOrValue::Value(())
    }
}

fn settle_transfer(hos_extension: &AccountId, settlement: PendingBuy) -> Promise {
    ext_hos_extension::ext(hos_extension.clone())
        .with_static_gas(GAS_FOR_FORCE_TRANSFER)
        .force_transfer(
            settlement.sub_account.clone(),
            Some(settlement.buyer.clone()),
            false,
        )
        .then(
            TlaRegistry::ext(env::current_account_id())
                .with_static_gas(GAS_FOR_SOLD_CALLBACK)
                .on_sub_account_sold(
                    settlement.tla_id,
                    settlement.name,
                    settlement.buyer,
                    settlement.price,
                    settlement.deposit,
                ),
        )
}

fn settle_transfer_paid(
    hos_extension: &AccountId,
    settlement: PendingBuy,
    operator: AccountId,
) -> Promise {
    ext_hos_extension::ext(hos_extension.clone())
        .with_static_gas(GAS_FOR_FORCE_TRANSFER)
        .force_transfer(
            settlement.sub_account.clone(),
            Some(settlement.buyer.clone()),
            false,
        )
        .then(
            TlaRegistry::ext(env::current_account_id())
                .with_static_gas(GAS_FOR_SOLD_CALLBACK)
                .on_sub_account_sold_paid(
                    settlement.tla_id,
                    settlement.name,
                    settlement.buyer,
                    operator,
                    settlement.price,
                ),
        )
}
