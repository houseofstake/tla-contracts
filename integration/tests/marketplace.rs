mod common;

use anyhow::{bail, Result};
use common::*;
use near_sdk::json_types::{U128, U64};
use near_workspaces::network::Sandbox;
use near_workspaces::types::NearToken;
use near_workspaces::{AccountId, Contract, Worker};
use serde_json::json;

/// Recovery watchers still hold real ed25519 keys; leased accounts do not.
const WATCHER_KEY: &str = "ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847";
const NEAR_USD_MICRO: u128 = 5_000_000;
const GRACE_NS: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;

async fn deploy_registry(fleet: &Fleet) -> Result<Contract> {
    let extension = fleet
        .extension
        .deploy(&wasm("hos_extension"))
        .await?
        .into_result()?;
    extension
        .call("new")
        .args_json(json!({
            "admin": fleet.council.id(),
            "registry": fleet.registry.id(),
            "recovery": fleet.recovery.id(),
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let recovery = fleet
        .recovery
        .deploy(&wasm("mpc_recovery"))
        .await?
        .into_result()?;
    recovery
        .call("new")
        .args_json(json!({
            "owner": fleet.council.id(),
            "signer": fleet.bob.id(),
            "transfer_authority": fleet.extension.id(),
            "watchers": [WATCHER_KEY],
            "threshold": 1,
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let registry = fleet
        .registry
        .deploy(&wasm("tla_registry"))
        .await?
        .into_result()?;
    registry
        .call("new")
        .args_json(json!({
            "admin": fleet.council.id(),
            "hos_extension": fleet.extension.id(),
            "grace_period_ns": U64(GRACE_NS),
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let tla = fleet.registrar.id();
    fleet
        .council
        .call(registry.id(), "admin_set_initial_rate")
        .args_json(json!({ "rate": U128(NEAR_USD_MICRO) }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    fleet
        .council
        .call(registry.id(), "register_tla")
        .args_json(json!({
            "tla_id": tla,
            "tla_type": "Open",
            "premium_category": "Standard",
            "licensee": null,
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    fleet
        .council
        .call(registry.id(), "activate_open_tla")
        .args_json(json!({ "tla_id": tla }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let mut fee: serde_json::Value = registry.view("get_fee_config").await?.json()?;
    fee["account_creation_deposit_yocto"] =
        json!(NearToken::from_millinear(200).as_yoctonear().to_string());
    fleet
        .council
        .call(registry.id(), "update_fee_config")
        .args_json(json!({ "config": fee }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    Ok(registry)
}

async fn rent_price(registry: &Contract, tla: &AccountId, name: &str) -> Result<u128> {
    let price: serde_json::Value = registry
        .view("get_rent_price")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    Ok(price["total_yocto"].as_str().unwrap().parse()?)
}

/// Waits until the minted wallet answers a standard view, which is the
/// readiness signal now that no access key is installed at mint.
async fn settled_wallet(worker: &Worker<Sandbox>, id: &AccountId) -> Result<Vec<String>> {
    for _ in 0..15 {
        if let Ok(result) = worker.view(id, "w_extensions").await {
            if let Ok(extensions) = result.json::<Vec<String>>() {
                if !extensions.is_empty() {
                    return Ok(extensions);
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    bail!("{id} never became queryable as a wallet contract")
}

async fn owner_key(worker: &Worker<Sandbox>, id: &AccountId) -> Result<String> {
    Ok(worker.view(id, "w_public_key").await?.json::<String>()?)
}

async fn owner_account(
    worker: &Worker<Sandbox>,
    id: &AccountId,
    authority: &AccountId,
) -> Result<String> {
    let extensions: Vec<String> = worker.view(id, "w_extensions").await?.json()?;
    Ok(extensions
        .into_iter()
        .find(|held| held != authority.as_str())
        .unwrap_or_default())
}

#[tokio::test]
async fn tla_registry_rent_drives_registrar_wallet_mint() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    let name = "alice";
    let total = rent_price(&registry, &tla, name).await?;

    let outcome = fleet
        .bob
        .call(registry.id(), "rent_sub_account")
        .args_json(json!({
            "tla_id": tla,
            "name": name,
        }))
        .deposit(NearToken::from_yoctonear(
            total + NearToken::from_millinear(100).as_yoctonear(),
        ))
        .max_gas()
        .transact()
        .await?;
    let outcome = outcome.into_result()?;
    if let Some(failure) = outcome.receipt_failures().first() {
        bail!("mint receipt failed: {failure:?}");
    }

    let tenant: AccountId = format!("{name}.{tla}").parse()?;
    let extensions = settled_wallet(&fleet.worker, &tenant).await?;
    assert_eq!(
        extensions.len(),
        2,
        "tenant must carry exactly the owner and the authority, got {extensions:?}"
    );
    assert!(
        fleet.worker.view_access_keys(&tenant).await?.is_empty(),
        "a leased account must carry no access key"
    );
    assert_eq!(
        owner_key(&fleet.worker, &tenant).await?,
        "",
        "a leased account must report no public key at all"
    );

    let lease: serde_json::Value = fleet.worker.view(&tenant, "hos_lease").await?.json()?;
    assert_eq!(lease["state"], "Active", "tenant wallet must be Active");

    Ok(())
}

async fn rent(
    fleet: &Fleet,
    registry: &Contract,
    tla: &AccountId,
    name: &str,
) -> Result<AccountId> {
    let total = rent_price(registry, tla, name).await?;
    let out = fleet
        .bob
        .call(registry.id(), "rent_sub_account")
        .args_json(json!({
            "tla_id": tla,
            "name": name,
        }))
        .deposit(NearToken::from_yoctonear(
            total + NearToken::from_millinear(100).as_yoctonear(),
        ))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = out.receipt_failures().first() {
        bail!("rent receipt failed: {failure:?}");
    }
    let tenant: AccountId = format!("{name}.{tla}").parse()?;
    settled_wallet(&fleet.worker, &tenant).await?;
    Ok(tenant)
}

#[tokio::test]
async fn tla_registry_list_and_buy_rotates_wallet_owner() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;

    let price = NearToken::from_near(1).as_yoctonear();
    let listed = fleet
        .bob
        .call(registry.id(), "list_sub_account")
        .args_json(json!({
            "tla_id": tla,
            "name": name,
            "price": price.to_string(),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = listed.receipt_failures().first() {
        bail!("list receipt failed: {failure:?}");
    }
    let listing: serde_json::Value = registry
        .view("get_listing")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert!(
        !listing.is_null(),
        "sub-account must be listed, got {listing}"
    );

    let bought = fleet
        .relay
        .call(registry.id(), "buy_sub_account")
        .args_json(json!({
            "tla_id": tla,
            "name": name,
        }))
        .deposit(NearToken::from_yoctonear(
            price + NearToken::from_millinear(100).as_yoctonear(),
        ))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = bought.receipt_failures().first() {
        bail!("buy receipt failed: {failure:?}");
    }

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.relay.id().as_str(),
        "the owner extension must rotate to the buyer after sale"
    );
    let sub: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert_eq!(
        sub["owner"],
        fleet.relay.id().as_str(),
        "registry owner must become the buyer after sale"
    );

    Ok(())
}

#[tokio::test]
async fn tla_registry_transfer_rotates_wallet_owner_without_a_sale() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;

    let moved = fleet
        .bob
        .call(registry.id(), "transfer_sub_account")
        .args_json(json!({
            "tla_id": tla,
            "name": name,
            "new_owner": fleet.relay.id(),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = moved.receipt_failures().first() {
        bail!("transfer receipt failed: {failure:?}");
    }

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.relay.id().as_str(),
        "the owner extension must rotate to the recipient"
    );
    let allowed: bool = fleet
        .worker
        .view(&tenant, "w_is_signature_allowed")
        .await?
        .json()?;
    assert!(
        !allowed,
        "a transferred name must stop honouring the previous owner's key"
    );
    let sub: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert_eq!(
        sub["owner"],
        fleet.relay.id().as_str(),
        "registry owner must become the recipient"
    );
    assert_eq!(
        sub["payout_account"],
        fleet.relay.id().as_str(),
        "main wallet must follow the new owner"
    );

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.relay.id().as_str(),
        "the wallet's owner extension must follow the registry transfer"
    );
    Ok(())
}

#[tokio::test]
async fn tla_registry_transfer_rejects_a_stale_owner_key() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;

    let attempted = fleet
        .relay
        .call(registry.id(), "transfer_sub_account")
        .args_json(json!({
            "tla_id": tla,
            "name": name,
            "new_owner": fleet.relay.id(),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    assert!(
        attempted.is_failure(),
        "a caller who does not own the name must not be able to transfer it: {attempted:#?}"
    );

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.bob.id().as_str(),
        "a refused transfer must leave the owner extension untouched"
    );
    Ok(())
}
