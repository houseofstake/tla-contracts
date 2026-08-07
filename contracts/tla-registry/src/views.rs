use crate::error::ContractError;
use crate::fees;
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use near_sdk::json_types::{U128, U64};
use near_sdk::serde::Serialize;
use near_sdk::store::IterableSet;
use near_sdk::{near, AccountId};

#[near]
impl TlaRegistry {
    pub fn get_tla(&self, tla_id: AccountId) -> Option<TlaView> {
        self.tlas
            .get(&tla_id)
            .map(|e| to_tla_view(&tla_id, e, &self.fee_config, &self.clock()))
    }

    pub fn get_sub_account(&self, tla_id: AccountId, name: String) -> Option<SubAccountView> {
        let key = sub_account_key(&tla_id, &name);
        let sub = self.sub_accounts.get(&key)?;
        let tla = self.tlas.get(&tla_id)?;
        Some(to_sub_view(
            &key,
            sub,
            tla,
            &tla_id,
            &name,
            &self.fee_config,
            &self.clock(),
        ))
    }

    pub fn get_parked_sub_account(
        &self,
        tla_id: AccountId,
        name: String,
    ) -> Option<ParkedSubAccountView> {
        let key = sub_account_key(&tla_id, &name);
        self.parked_names.get(&key).map(|p| ParkedSubAccountView {
            full_name: key.clone(),
            tla_id: p.tla_id.clone(),
            parked_at: U64(p.parked_at),
        })
    }

    pub fn is_name_re_rentable(&self, tla_id: AccountId, name: String) -> bool {
        let key = sub_account_key(&tla_id, &name);
        self.parked_names.contains_key(&key)
    }

    pub fn is_reclaim_in_progress(&self, tla_id: AccountId, name: String) -> bool {
        let key = sub_account_key(&tla_id, &name);
        self.reclaim_pending.contains_key(&key)
    }

    pub fn get_hos_extension(&self) -> AccountId {
        self.hos_extension.clone()
    }

    pub fn get_grace_period_ns(&self) -> U64 {
        U64(self.grace_period_ns)
    }

    #[handle_result]
    pub fn get_rent_price(
        &self,
        tla_id: AccountId,
        name: String,
    ) -> Result<RentPriceView, ContractError> {
        let tla = self.tlas.get(&tla_id).ok_or(ContractError::TlaNotFound)?;
        let rent_usd = fees::calculate_rent(tla, &tla_id, &name, &self.fee_config);
        let rent_quoted = self.quote_usd_to_near(rent_usd)?;
        let key = sub_account_key(&tla_id, &name);
        let deposit = if self.parked_names.contains_key(&key) {
            0
        } else {
            self.fee_config.account_creation_deposit_yocto.0
        };
        Ok(RentPriceView {
            rent_yocto: U128(rent_quoted),
            creation_deposit_yocto: U128(deposit),
            total_yocto: U128(rent_quoted.saturating_add(deposit)),
        })
    }

    pub fn is_name_available(&self, tla_id: AccountId, name: String) -> bool {
        let key = sub_account_key(&tla_id, &name);
        !self.sub_accounts.contains_key(&key)
    }

    pub fn list_tlas(&self, from_index: u64, limit: u64) -> Vec<TlaView> {
        self.tlas
            .iter()
            .skip(from_index as usize)
            .take(limit as usize)
            .map(|(id, entry)| to_tla_view(id, entry, &self.fee_config, &self.clock()))
            .collect()
    }

    pub fn list_sub_accounts(&self, from_index: u64, limit: u64) -> Vec<SubAccountDetailView> {
        self.sub_accounts
            .iter()
            .skip(from_index as usize)
            .take(limit as usize)
            .filter_map(|(key, sub)| self.to_sub_detail(key, sub))
            .collect()
    }

    pub fn list_sub_accounts_by_owner(
        &self,
        owner: AccountId,
        from_index: u64,
        limit: u64,
    ) -> Vec<SubAccountDetailView> {
        let Some(keys) = self.sub_accounts_by_owner.get(&owner) else {
            return Vec::new();
        };
        self.page_details(keys, from_index, limit)
    }

    pub fn list_sub_accounts_by_tla(
        &self,
        tla_id: AccountId,
        from_index: u64,
        limit: u64,
    ) -> Vec<SubAccountDetailView> {
        let Some(keys) = self.sub_accounts_by_tla.get(&tla_id) else {
            return Vec::new();
        };
        self.page_details(keys, from_index, limit)
    }

    pub fn list_listings(&self, from_index: u64, limit: u64) -> Vec<ListingView> {
        self.listings
            .iter()
            .skip(from_index as usize)
            .take(limit as usize)
            .map(|(key, listing)| ListingView {
                full_name: key.clone(),
                price_yocto: U128(listing.price),
                settling: listing.settling,
            })
            .collect()
    }

    pub fn list_recent_activity(
        &self,
        from_index: u64,
        limit: u64,
        account: Option<String>,
    ) -> Vec<ActivityView> {
        let len = u64::from(self.recent_activity.len());
        (0..len)
            .filter_map(|i| {
                let slot = (u64::from(self.activity_cursor) + len - 1 - i) % len;
                self.recent_activity.get(slot as u32)
            })
            .filter(|r| account.as_ref().is_none_or(|a| &r.account == a))
            .skip(from_index as usize)
            .take(limit as usize)
            .map(|r| ActivityView {
                event: r.event.clone(),
                account: r.account.clone(),
                block_height: U64(r.block_height),
                block_timestamp: U64(r.block_timestamp),
            })
            .collect()
    }

    pub fn get_fee_config(&self) -> FeeConfig {
        self.fee_config.clone()
    }

    pub fn get_stats(&self) -> RegistryStats {
        RegistryStats {
            tla_count: u64::from(self.tlas.len()),
            sub_account_count: self.sub_account_count,
            total_revenue_yocto: U128(self.total_revenue),
            total_pending_refunds_yocto: U128(self.total_pending_refunds),
        }
    }

    pub fn get_admins(&self) -> Vec<AccountId> {
        self.admins.iter().cloned().collect()
    }

    pub fn get_payment_authorities(&self) -> Vec<AccountId> {
        self.payment_authorities.iter().cloned().collect()
    }

    pub fn get_recovery_authorities(&self) -> Vec<AccountId> {
        self.recovery_authorities.iter().cloned().collect()
    }

    pub fn get_ft_allowlist(&self) -> Vec<AccountId> {
        self.ft_allowlist.iter().cloned().collect()
    }
}

impl TlaRegistry {
    fn page_details(
        &self,
        keys: &IterableSet<String>,
        from_index: u64,
        limit: u64,
    ) -> Vec<SubAccountDetailView> {
        keys.iter()
            .skip(from_index as usize)
            .take(limit as usize)
            .filter_map(|key| self.to_sub_detail(key, self.sub_accounts.get(key)?))
            .collect()
    }

    fn to_sub_detail(&self, key: &str, sub: &SubAccountEntry) -> Option<SubAccountDetailView> {
        let tla = self.tlas.get(&sub.tla_id)?;
        let name = key.strip_suffix(&format!(".{}", sub.tla_id))?;
        Some(SubAccountDetailView {
            sub_account: to_sub_view(
                key,
                sub,
                tla,
                &sub.tla_id,
                name,
                &self.fee_config,
                &self.clock(),
            ),
            listing: self.listings.get(key).map(|l| ListingView {
                full_name: key.to_string(),
                price_yocto: U128(l.price),
                settling: l.settling,
            }),
            accepted_offer: self.accepted_offers.get(key).map(|o| AcceptedOfferView {
                full_name: key.to_string(),
                buyer: o.buyer.clone(),
                price_yocto: U128(o.price),
                settling: o.settling,
            }),
            retraction_at: sub.retraction_at.map(U64),
        })
    }
}

pub(crate) fn to_tla_view(
    tla_id: &AccountId,
    entry: &TlaEntry,
    config: &FeeConfig,
    clock: &LifecycleClock,
) -> TlaView {
    let tla_len = tla_id.as_str().len() as u8;
    let rent = fees::base_rent(tla_len, config);
    TlaView {
        tla_id: tla_id.clone(),
        tla_type: entry.tla_type.clone(),
        lifecycle: entry.lifecycle(clock),
        licensee: entry.licensee.clone(),
        premium_category: entry.premium_category.clone(),
        activated_at: U64(entry.activated_at),
        expires_at: U64(entry.expires_at),
        annual_rent: U128(rent),
    }
}

pub(crate) fn to_sub_view(
    key: &str,
    entry: &SubAccountEntry,
    tla: &TlaEntry,
    tla_id: &AccountId,
    name: &str,
    config: &FeeConfig,
    clock: &LifecycleClock,
) -> SubAccountView {
    let rent = fees::calculate_rent(tla, tla_id, name, config);
    SubAccountView {
        full_name: key.to_string(),
        owner: entry.owner.clone(),
        tla_id: entry.tla_id.clone(),
        payout_account: entry.payout_account.clone(),
        lifecycle: entry.lifecycle(clock),
        rented_at: U64(entry.rented_at),
        expires_at: U64(entry.expires_at),
        annual_rent: U128(rent),
    }
}

#[derive(Serialize)]
#[serde(crate = "near_sdk::serde")]
pub struct ParkedSubAccountView {
    pub full_name: String,
    pub tla_id: AccountId,
    pub parked_at: U64,
}
