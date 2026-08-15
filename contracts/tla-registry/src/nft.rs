use crate::error::ContractError;
use crate::events::Event;
use crate::interfaces::ext_hos_extension;
use crate::marketplace::{GAS_FOR_FORCE_TRANSFER, GAS_FOR_TRANSFER_CALLBACK};
use crate::types::*;
use crate::{TlaRegistry, TlaRegistryExt};
use hos_common::RotationCause;
use near_sdk::json_types::U128;
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::serde_json::{json, Value};
use near_sdk::{
    env, ext_contract, near, AccountId, FunctionError, Gas, Promise, PromiseError, PromiseOrValue,
};

const EVENT_STANDARD: &str = "nep171";
const EVENT_VERSION: &str = "1.0.0";
const GAS_FOR_NFT_ON_TRANSFER: Gas = Gas::from_tgas(25);
/// Must fund the return rotation it may schedule: a force_transfer plus its
/// callback, not just its own execution.
const GAS_FOR_RESOLVE_TRANSFER: Gas = Gas::from_tgas(80);

#[derive(Serialize, Deserialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct Token {
    pub token_id: String,
    pub owner_id: AccountId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<TokenMetadata>,
}

#[near(serializers = [json])]
pub struct NftTransferCall {
    pub tla_id: AccountId,
    pub name: String,
    pub from: AccountId,
    pub to: AccountId,
    pub memo: Option<String>,
    pub msg: String,
}

#[allow(dead_code)]
#[ext_contract(ext_nft_receiver)]
pub trait NonFungibleTokenReceiver {
    fn nft_on_transfer(
        &mut self,
        sender_id: AccountId,
        previous_owner_id: AccountId,
        token_id: String,
        msg: String,
    ) -> PromiseOrValue<bool>;
}

fn log_nft_event(event: &str, data: Value) {
    let payload = json!({
        "standard": EVENT_STANDARD,
        "version": EVENT_VERSION,
        "event": event,
        "data": [data],
    });
    env::log_str(&format!("EVENT_JSON:{payload}"));
}

pub(crate) fn emit_nft_mint(owner_id: &AccountId, token_id: &str) {
    log_nft_event(
        "nft_mint",
        json!({ "owner_id": owner_id, "token_ids": [token_id] }),
    );
}

fn emit_nft_transfer(
    old_owner_id: &AccountId,
    new_owner_id: &AccountId,
    token_id: &str,
    memo: Option<&String>,
) {
    let mut data = json!({
        "old_owner_id": old_owner_id,
        "new_owner_id": new_owner_id,
        "token_ids": [token_id],
    });
    if let Some(memo) = memo {
        data["memo"] = Value::String(memo.clone());
    }
    log_nft_event("nft_transfer", data);
}

#[near]
impl TlaRegistry {
    #[handle_result]
    #[payable]
    pub fn nft_transfer(
        &mut self,
        receiver_id: AccountId,
        token_id: String,
        approval_id: Option<u64>,
        memo: Option<String>,
    ) -> Result<Promise, ContractError> {
        let (tla_id, name, from, sub_account) =
            self.open_nft_transfer(&receiver_id, &token_id, approval_id)?;
        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_FORCE_TRANSFER)
            .force_transfer(
                sub_account,
                Some(receiver_id.clone()),
                RotationCause::Transfer,
                Some(from.clone()),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_TRANSFER_CALLBACK)
                    .nft_on_rotation_resolved(tla_id, name, from, receiver_id, memo),
            ))
    }

    #[handle_result]
    #[payable]
    pub fn nft_transfer_call(
        &mut self,
        receiver_id: AccountId,
        token_id: String,
        approval_id: Option<u64>,
        memo: Option<String>,
        msg: String,
    ) -> Result<Promise, ContractError> {
        // A market pause closes the entrance, never the exit. Handing a name to
        // a receiver is how it is deposited; taking it back out is a plain
        // nft_transfer, which stays open so a pause can never strand it there.
        self.assert_marketplace_open()?;
        let (tla_id, name, from, sub_account) =
            self.open_nft_transfer(&receiver_id, &token_id, approval_id)?;
        Ok(ext_hos_extension::ext(self.hos_extension.clone())
            .with_static_gas(GAS_FOR_FORCE_TRANSFER)
            .force_transfer(
                sub_account,
                Some(receiver_id.clone()),
                RotationCause::Transfer,
                Some(from.clone()),
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(
                        GAS_FOR_TRANSFER_CALLBACK
                            .saturating_add(GAS_FOR_NFT_ON_TRANSFER)
                            .saturating_add(GAS_FOR_RESOLVE_TRANSFER),
                    )
                    .nft_on_rotation_resolved_call(NftTransferCall {
                        tla_id,
                        name,
                        from,
                        to: receiver_id,
                        memo,
                        msg,
                    }),
            ))
    }

    /// The rotation is the transfer. A failed rotation must fail this receipt
    /// rather than return quietly, because callers such as NEAR Intents read an
    /// empty successful result as proof the token moved and burn their own
    /// balance against it.
    #[private]
    pub fn nft_on_rotation_resolved(
        &mut self,
        tla_id: AccountId,
        name: String,
        from: AccountId,
        to: AccountId,
        memo: Option<String>,
        #[callback_result] swapped: Result<bool, PromiseError>,
    ) {
        self.commit_nft_rotation(&tla_id, &name, &from, &to, memo.as_ref(), swapped);
    }

    #[private]
    pub fn nft_on_rotation_resolved_call(
        &mut self,
        call: NftTransferCall,
        #[callback_result] swapped: Result<bool, PromiseError>,
    ) -> Promise {
        let NftTransferCall {
            tla_id,
            name,
            from,
            to,
            memo,
            msg,
        } = call;
        let token_id = sub_account_key(&tla_id, &name);
        self.commit_nft_rotation(&tla_id, &name, &from, &to, memo.as_ref(), swapped);
        ext_nft_receiver::ext(to.clone())
            .with_static_gas(GAS_FOR_NFT_ON_TRANSFER)
            .with_unused_gas_weight(1)
            .nft_on_transfer(from.clone(), from.clone(), token_id, msg)
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_RESOLVE_TRANSFER)
                    .nft_resolve_transfer(tla_id, name, from, to),
            )
    }

    #[private]
    pub fn nft_resolve_transfer(
        &mut self,
        tla_id: AccountId,
        name: String,
        from: AccountId,
        to: AccountId,
        #[callback_result] returned: Result<bool, PromiseError>,
    ) -> PromiseOrValue<bool> {
        let give_back = returned.unwrap_or(true);
        if !give_back {
            return PromiseOrValue::Value(true);
        }
        let key = sub_account_key(&tla_id, &name);
        if self.sub_accounts.get(&key).map(|sub| &sub.owner) != Some(&to) {
            return PromiseOrValue::Value(true);
        }
        let Ok(sub_account) = key.parse::<AccountId>() else {
            return PromiseOrValue::Value(true);
        };
        PromiseOrValue::Promise(
            ext_hos_extension::ext(self.hos_extension.clone())
                .with_static_gas(GAS_FOR_FORCE_TRANSFER)
                .force_transfer(sub_account, Some(from.clone()), RotationCause::Revert, None)
                .then(
                    Self::ext(env::current_account_id())
                        .with_static_gas(GAS_FOR_TRANSFER_CALLBACK)
                        .nft_on_return_resolved(tla_id, name, to, from),
                ),
        )
    }

    /// The forward transfer already happened and was reported, so a failed
    /// give-back cannot be undone by panicking here. Record it instead, or the
    /// only evidence that a name is stuck with a receiver is lost.
    #[private]
    pub fn nft_on_return_resolved(
        &mut self,
        tla_id: AccountId,
        name: String,
        from: AccountId,
        to: AccountId,
        #[callback_result] swapped: Result<bool, PromiseError>,
    ) -> bool {
        let key = sub_account_key(&tla_id, &name);
        if !matches!(swapped, Ok(true)) || !self.sub_account_reassign(&key, &to, &to) {
            Event::TransferFailed {
                full_name: key,
                from,
                to,
            }
            .emit();
            return false;
        }
        emit_nft_transfer(&from, &to, &key, None);
        self.emit_activity(Event::SubAccountTransferred {
            full_name: key,
            tla_id,
            from,
            to,
        });
        false
    }

    pub fn nft_token(&self, token_id: String) -> Option<Token> {
        let sub = self.sub_accounts.get(&token_id)?;
        Some(token_of(&token_id, sub))
    }

    pub fn nft_metadata(&self) -> NftContractMetadata {
        self.nft_contract_metadata.clone()
    }

    #[handle_result]
    #[payable]
    pub fn admin_set_nft_metadata(
        &mut self,
        name: String,
        symbol: String,
        icon: Option<String>,
        base_uri: Option<String>,
        reference: Option<String>,
    ) -> Result<(), ContractError> {
        crate::assert_one_yocto()?;
        self.assert_admin()?;
        self.nft_contract_metadata = NftContractMetadata {
            spec: NFT_SPEC.to_string(),
            name,
            symbol,
            icon,
            base_uri,
            reference,
            reference_hash: None,
        };
        Ok(())
    }

    pub fn nft_total_supply(&self) -> U128 {
        U128(u128::from(self.sub_account_count))
    }

    pub fn nft_supply_for_owner(&self, account_id: AccountId) -> U128 {
        U128(
            self.sub_accounts_by_owner
                .get(&account_id)
                .map_or(0, |keys| {
                    keys.iter()
                        .filter(|key| self.sub_accounts.contains_key(*key))
                        .count() as u128
                }),
        )
    }

    pub fn nft_tokens(&self, from_index: Option<U128>, limit: Option<u64>) -> Vec<Token> {
        let start = from_index.map_or(0, |i| i.0 as usize);
        self.sub_accounts
            .iter()
            .skip(start)
            .take(limit.unwrap_or(MAX_PAGE_LIMIT).min(MAX_PAGE_LIMIT) as usize)
            .map(|(key, sub)| token_of(key, sub))
            .collect()
    }

    pub fn nft_tokens_for_owner(
        &self,
        account_id: AccountId,
        from_index: Option<U128>,
        limit: Option<u64>,
    ) -> Vec<Token> {
        let Some(keys) = self.sub_accounts_by_owner.get(&account_id) else {
            return Vec::new();
        };
        let start = from_index.map_or(0, |i| i.0 as usize);
        keys.iter()
            .skip(start)
            .take(limit.unwrap_or(MAX_PAGE_LIMIT).min(MAX_PAGE_LIMIT) as usize)
            .filter_map(|key| Some(token_of(key, self.sub_accounts.get(key)?)))
            .collect()
    }
}

pub(crate) fn initial_metadata() -> NftContractMetadata {
    let account = env::current_account_id().to_string();
    NftContractMetadata {
        spec: NFT_SPEC.to_string(),
        name: account.clone(),
        symbol: account,
        icon: None,
        base_uri: None,
        reference: None,
        reference_hash: None,
    }
}

fn token_of(token_id: &str, sub: &SubAccountEntry) -> Token {
    Token {
        token_id: token_id.to_owned(),
        owner_id: sub.owner.clone(),
        metadata: Some(TokenMetadata {
            title: Some(token_id.to_owned()),
            description: None,
            media: None,
            media_hash: None,
            copies: Some(1),
            issued_at: None,
            expires_at: None,
            starts_at: None,
            updated_at: None,
            extra: Some(
                json!({
                    "rented_at": sub.rented_at.to_string(),
                    "expires_at": sub.expires_at.to_string(),
                })
                .to_string(),
            ),
            reference: None,
            reference_hash: None,
        }),
    }
}

impl TlaRegistry {
    fn open_nft_transfer(
        &mut self,
        receiver_id: &AccountId,
        token_id: &str,
        approval_id: Option<u64>,
    ) -> Result<(AccountId, String, AccountId, AccountId), ContractError> {
        if approval_id.is_some() {
            return Err(ContractError::ApprovalsNotSupported);
        }
        let (tla_id, name) = self.split_token_id(token_id)?;
        let (sub_account, from) = self.assert_transferable(&tla_id, &name, receiver_id)?;
        Ok((tla_id, name, from, sub_account))
    }

    fn commit_nft_rotation(
        &mut self,
        tla_id: &AccountId,
        name: &str,
        from: &AccountId,
        to: &AccountId,
        memo: Option<&String>,
        swapped: Result<bool, PromiseError>,
    ) {
        if !matches!(swapped, Ok(true)) {
            ContractError::RotationNotConfirmed.panic();
        }
        let key = sub_account_key(tla_id, name);
        if !self.sub_account_reassign(&key, to, to) {
            ContractError::OwnerIndexOutOfSync.panic();
        }
        emit_nft_transfer(from, to, &key, memo);
        self.emit_activity(Event::SubAccountTransferred {
            full_name: key,
            tla_id: tla_id.clone(),
            from: from.clone(),
            to: to.clone(),
        });
    }

    fn split_token_id(&self, token_id: &str) -> Result<(AccountId, String), ContractError> {
        let sub = self
            .sub_accounts
            .get(token_id)
            .ok_or(ContractError::TokenNotFound)?;
        let tla_id = sub.tla_id.clone();
        let name = token_id
            .strip_suffix(&format!(".{tla_id}"))
            .ok_or(ContractError::TokenNotFound)?
            .to_string();
        Ok((tla_id, name))
    }
}
