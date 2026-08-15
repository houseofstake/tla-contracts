mod common;

use anyhow::{bail, Result};
use common::*;
use near_sdk::json_types::U64;
use near_workspaces::network::Sandbox;
use near_workspaces::types::NearToken;
use near_workspaces::{AccountId, Contract, Worker};
use serde_json::json;

#[tokio::test]
async fn an_item_and_the_collection_agree_and_a_forgery_does_not() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let tenant = rent(&fleet, &registry, &tla, "alice").await?;

    let item: serde_json::Value = fleet.worker.view(&tenant, "nft_item_info").await?.json()?;
    assert_eq!(item["init"], true);
    assert_eq!(item["collection_id"], registry.id().as_str());
    assert_eq!(item["token_id"], tenant.as_str());
    assert!(
        tenant.as_str().ends_with(&format!(".{tla}")),
        "the caller derives the parent from the account it queried, not from a field"
    );

    let token: serde_json::Value = registry
        .view("nft_token")
        .args_json(json!({ "token_id": item["token_id"] }))
        .await?
        .json()?;
    assert_eq!(
        item["owner_id"], token["owner_id"],
        "the item and the collection must name the same owner"
    );

    let forger = fleet
        .relay
        .create_subaccount("evil")
        .initial_balance(NearToken::from_near(6))
        .transact()
        .await?
        .into_result()?;
    let forgery = forger.deploy(&wasm("hos_wallet")).await?.into_result()?;
    fleet
        .relay
        .call(forgery.id(), "hos_init")
        .args_json(json!({ "config": {
            "owner_account": fleet.relay.id(),
            "authority": fleet.extension.id(),
            "collection_id": registry.id(),
            "payout_account": fleet.relay.id(),
            "lease_until_ns": U64(lease_until_ns()),
            "timeout_secs": 3600,
            "recovery": fleet.recovery.id(),
        }}))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let claim: serde_json::Value = fleet
        .worker
        .view(forgery.id(), "nft_item_info")
        .await?
        .json()?;
    assert_eq!(
        claim["collection_id"],
        registry.id().as_str(),
        "the forgery is free to claim any collection it likes"
    );
    assert!(
        !forgery.id().as_str().ends_with(&format!(".{tla}")),
        "but its account cannot sit under a parent that never created it"
    );
    let absent: serde_json::Value = registry
        .view("nft_token")
        .args_json(json!({ "token_id": claim["token_id"] }))
        .await?
        .json()?;
    assert!(
        absent.is_null(),
        "and the collection it named has never heard of it"
    );

    Ok(())
}

async fn item_info(worker: &Worker<Sandbox>, id: &AccountId) -> Result<serde_json::Value> {
    Ok(worker.view(id, "nft_item_info").await?.json()?)
}

async fn collection_says(registry: &Contract, token_id: &str) -> Result<serde_json::Value> {
    Ok(registry
        .view("nft_token")
        .args_json(json!({ "token_id": token_id }))
        .await?
        .json()?)
}

fn pair_accepts(
    account: &AccountId,
    item: &serde_json::Value,
    token: &serde_json::Value,
    collection: &AccountId,
) -> bool {
    if token.is_null() || !item["init"].as_bool().unwrap_or(false) {
        return false;
    }
    item["collection_id"] == collection.as_str()
        && item["token_id"] == account.as_str()
        && token["token_id"] == item["token_id"]
        && token["owner_id"] == item["owner_id"]
}

#[tokio::test]
async fn a_forgery_created_under_the_real_parent_is_still_refused() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let honest = rent(&fleet, &registry, &tla, "alice").await?;

    let item = item_info(&fleet.worker, &honest).await?;
    let token = collection_says(&registry, item["token_id"].as_str().unwrap()).await?;
    assert!(
        pair_accepts(&honest, &item, &token, registry.id()),
        "a name the registrar minted must pass"
    );

    let evil = fleet
        .registrar
        .as_account()
        .create_subaccount("evil")
        .initial_balance(NearToken::from_near(6))
        .transact()
        .await?
        .into_result()?;
    let forgery = evil.deploy(&wasm("hos_wallet")).await?.into_result()?;
    fleet
        .registrar
        .as_account()
        .call(forgery.id(), "hos_init")
        .args_json(json!({ "config": {
            "owner_account": fleet.relay.id(),
            "authority": fleet.extension.id(),
            "collection_id": registry.id(),
            "payout_account": fleet.relay.id(),
            "lease_until_ns": U64(lease_until_ns()),
            "timeout_secs": 3600,
            "recovery": fleet.recovery.id(),
        }}))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let claim = item_info(&fleet.worker, forgery.id()).await?;
    assert!(
        forgery.id().as_str().ends_with(&format!(".{tla}")),
        "the whole point of this case: the forgery sits under a genuine parent"
    );
    assert_eq!(claim["collection_id"], registry.id().as_str());
    assert_eq!(claim["token_id"], forgery.id().as_str());
    assert!(
        claim["init"].as_bool().unwrap(),
        "and it reports itself fully initialised"
    );

    let absent = collection_says(&registry, claim["token_id"].as_str().unwrap()).await?;
    assert!(
        !pair_accepts(forgery.id(), &claim, &absent, registry.id()),
        "nothing about the naming separates it, so the collection half must"
    );

    Ok(())
}

#[tokio::test]
async fn the_pair_refuses_an_item_and_a_collection_that_disagree() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let tenant = rent(&fleet, &registry, &tla, "alice").await?;

    let rotated = fleet
        .registry
        .call(fleet.extension.id(), "force_transfer")
        .args_json(json!({
            "wallet": tenant,
            "new_owner": fleet.relay.id(),
            "cause": "Recovery",
            "asked_by": null,
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = rotated.receipt_failures().first() {
        bail!("the split-brain setup failed: {failure:?}");
    }

    let item = item_info(&fleet.worker, &tenant).await?;
    let token = collection_says(&registry, item["token_id"].as_str().unwrap()).await?;
    assert_eq!(item["owner_id"], fleet.relay.id().as_str());
    assert_eq!(
        token["owner_id"],
        fleet.bob.id().as_str(),
        "the collection index was never told, which is the state being detected"
    );
    assert!(
        !pair_accepts(&tenant, &item, &token, registry.id()),
        "an item and a collection that name different owners must not be accepted"
    );

    Ok(())
}
