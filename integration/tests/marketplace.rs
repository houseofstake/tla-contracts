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
            "treasury": fleet.council.id(),
            "council": fleet.council.id(),
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
            "treasury": fleet.council.id(),
            "council": fleet.council.id(),
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
async fn nft_transfer_rotates_the_wallet_and_the_registry_together() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    arm_transfer(&fleet, &tenant).await?;
    let token_id = format!("{name}.{tla}");

    let moved = fleet
        .bob
        .call(registry.id(), "nft_transfer")
        .args_json(json!({
            "receiver_id": fleet.relay.id(),
            "token_id": token_id,
            "approval_id": null,
            "memo": null,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = moved.receipt_failures().first() {
        bail!("nft_transfer receipt failed: {failure:?}");
    }

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.relay.id().as_str(),
        "the owner extension must rotate to the receiver"
    );
    let sub: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert_eq!(
        sub["owner"],
        fleet.relay.id().as_str(),
        "registry owner must follow the wallet rotation"
    );
    let token: serde_json::Value = registry
        .view("nft_token")
        .args_json(json!({ "token_id": token_id }))
        .await?
        .json()?;
    assert_eq!(
        token["owner_id"],
        fleet.relay.id().as_str(),
        "nft_token must report the receiver"
    );

    Ok(())
}

#[tokio::test]
async fn nft_transfer_by_a_non_owner_moves_nothing() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    arm_transfer(&fleet, &tenant).await?;
    let token_id = format!("{name}.{tla}");

    let attempt = fleet
        .relay
        .call(registry.id(), "nft_transfer")
        .args_json(json!({
            "receiver_id": fleet.relay.id(),
            "token_id": token_id,
            "approval_id": null,
            "memo": null,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    assert!(
        attempt.into_result().is_err(),
        "a stranger must not be able to move a name"
    );
    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.bob.id().as_str(),
        "the wallet must still name the original owner"
    );

    Ok(())
}

async fn deploy_receiver(fleet: &Fleet) -> Result<Contract> {
    let account = fleet
        .relay
        .create_subaccount("receiver")
        .initial_balance(NearToken::from_near(10))
        .transact()
        .await?
        .into_result()?;
    let receiver = account.deploy(&wasm("test_dapp")).await?.into_result()?;
    receiver
        .call("new")
        .args_json(json!({ "token": fleet.relay.id() }))
        .transact()
        .await?
        .into_result()?;
    Ok(receiver)
}

async fn transfer_call(
    fleet: &Fleet,
    registry: &Contract,
    receiver: &Contract,
    token_id: &str,
    msg: &str,
) -> Result<near_workspaces::result::ExecutionFinalResult> {
    Ok(fleet
        .bob
        .call(registry.id(), "nft_transfer_call")
        .args_json(json!({
            "receiver_id": receiver.id(),
            "token_id": token_id,
            "approval_id": null,
            "memo": null,
            "msg": msg,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?)
}

#[tokio::test]
async fn nft_transfer_call_leaves_the_name_with_a_receiver_that_keeps_it() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let receiver = deploy_receiver(&fleet).await?;

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    arm_transfer(&fleet, &tenant).await?;
    let token_id = format!("{name}.{tla}");

    let out = transfer_call(&fleet, &registry, &receiver, &token_id, "keep").await?;
    let out = out.into_result()?;
    if let Some(failure) = out.receipt_failures().first() {
        bail!("transfer_call receipt failed: {failure:?}");
    }

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        receiver.id().as_str(),
        "a receiver that keeps the token must hold the wallet"
    );
    let token: serde_json::Value = registry
        .view("nft_token")
        .args_json(json!({ "token_id": token_id }))
        .await?
        .json()?;
    assert_eq!(token["owner_id"], receiver.id().as_str());
    Ok(())
}

#[tokio::test]
async fn nft_transfer_call_returns_the_name_when_the_receiver_refuses_it() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let receiver = deploy_receiver(&fleet).await?;

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    arm_transfer(&fleet, &tenant).await?;
    let token_id = format!("{name}.{tla}");

    transfer_call(&fleet, &registry, &receiver, &token_id, "return")
        .await?
        .into_result()?;

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.bob.id().as_str(),
        "a refused token must be rotated back to the original owner"
    );
    let token: serde_json::Value = registry
        .view("nft_token")
        .args_json(json!({ "token_id": token_id }))
        .await?
        .json()?;
    assert_eq!(
        token["owner_id"],
        fleet.bob.id().as_str(),
        "the registry must follow the rotation back"
    );
    Ok(())
}

#[tokio::test]
async fn nft_transfer_call_returns_the_name_when_the_receiver_panics() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let receiver = deploy_receiver(&fleet).await?;

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    arm_transfer(&fleet, &tenant).await?;
    let token_id = format!("{name}.{tla}");

    transfer_call(&fleet, &registry, &receiver, &token_id, "panic")
        .await?
        .into_result()?;

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.bob.id().as_str(),
        "a receiver that panics must not keep the name"
    );
    Ok(())
}

#[tokio::test]
async fn nft_transfer_refuses_an_approval_id() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    arm_transfer(&fleet, &tenant).await?;
    let token_id = format!("{name}.{tla}");

    let attempt = fleet
        .bob
        .call(registry.id(), "nft_transfer")
        .args_json(json!({
            "receiver_id": fleet.relay.id(),
            "token_id": token_id,
            "approval_id": 1u64,
            "memo": null,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    assert!(
        attempt.into_result().is_err(),
        "approvals are not implemented, so an approval_id must be refused rather than ignored"
    );
    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.bob.id().as_str(),
        "a refused transfer must leave the wallet untouched"
    );

    Ok(())
}

#[tokio::test]
async fn a_sale_pays_the_sellers_balance_out_with_the_name() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    fleet
        .council
        .transfer_near(&tenant, NearToken::from_near(3))
        .await?
        .into_result()?;
    arm_transfer(&fleet, &tenant).await?;

    let held = balance_of(&fleet.worker, &tenant).await?;
    let seller_before = balance_of(&fleet.worker, fleet.bob.id()).await?;

    let moved = fleet
        .bob
        .call(registry.id(), "nft_transfer")
        .args_json(json!({
            "receiver_id": fleet.relay.id(),
            "token_id": format!("{name}.{tla}"),
            "approval_id": null,
            "memo": null,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = moved.receipt_failures().first() {
        bail!("nft_transfer receipt failed: {failure:?}");
    }

    let left = balance_of(&fleet.worker, &tenant).await?;
    assert!(
        left < NearToken::from_millinear(500).as_yoctonear(),
        "the transferred account must not carry the seller's funds to the buyer, found {left} yocto"
    );
    let seller_after = balance_of(&fleet.worker, fleet.bob.id()).await?;
    assert!(
        seller_after > seller_before,
        "the seller must receive the account balance on the way out: \
         {seller_before} -> {seller_after} with {held} yocto held"
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
    arm_transfer(&fleet, &tenant).await?;

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

async fn detail_names(
    registry: &Contract,
    method: &str,
    args: serde_json::Value,
) -> Result<Vec<String>> {
    let page: Vec<serde_json::Value> = registry.view(method).args_json(args).await?.json()?;
    Ok(page
        .into_iter()
        .map(|d| {
            d["sub_account"]["full_name"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect())
}

#[tokio::test]
async fn tla_registry_paged_views_serve_the_catalogue_without_an_indexer() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    rent(&fleet, &registry, &tla, "alice").await?;
    rent(&fleet, &registry, &tla, "carol").await?;
    let owner = fleet.bob.id().clone();

    let browse = detail_names(
        &registry,
        "list_sub_accounts",
        json!({ "from_index": 0, "limit": 10 }),
    )
    .await?;
    assert_eq!(browse.len(), 2, "browse page must carry both rented names");
    assert!(browse.contains(&format!("alice.{tla}")));
    assert!(browse.contains(&format!("carol.{tla}")));

    let owned = detail_names(
        &registry,
        "list_sub_accounts_by_owner",
        json!({ "owner": owner, "from_index": 0, "limit": 10 }),
    )
    .await?;
    assert_eq!(owned.len(), 2, "the owner index must hold both names");

    let by_tla = detail_names(
        &registry,
        "list_sub_accounts_by_tla",
        json!({ "tla_id": tla, "from_index": 0, "limit": 10 }),
    )
    .await?;
    assert_eq!(by_tla.len(), 2, "the namespace index must hold both names");

    let stranger = detail_names(
        &registry,
        "list_sub_accounts_by_owner",
        json!({ "owner": fleet.council.id(), "from_index": 0, "limit": 10 }),
    )
    .await?;
    assert!(
        stranger.is_empty(),
        "an owner with no names must page empty"
    );

    let paged = detail_names(
        &registry,
        "list_sub_accounts_by_owner",
        json!({ "owner": owner, "from_index": 1, "limit": 10 }),
    )
    .await?;
    assert_eq!(paged.len(), 1, "owner paging must respect from_index");

    let tokens: Vec<serde_json::Value> = registry
        .view("nft_tokens_for_owner")
        .args_json(json!({ "account_id": owner, "from_index": null, "limit": null }))
        .await?
        .json()?;
    assert_eq!(
        tokens.len(),
        2,
        "the owner's names must be enumerable as tokens"
    );
    assert!(
        tokens
            .iter()
            .any(|t| t["token_id"] == format!("alice.{tla}")),
        "nft_tokens_for_owner must carry the name"
    );

    let supply: serde_json::Value = registry.view("nft_total_supply").await?.json()?;
    assert_eq!(supply, "2", "total supply must count every minted name");

    let scoped: Vec<serde_json::Value> = registry
        .view("list_recent_activity")
        .args_json(json!({
            "from_index": 0,
            "limit": 10,
            "account": format!("carol.{tla}"),
        }))
        .await?
        .json()?;
    assert_eq!(
        scoped.len(),
        1,
        "the registry scopes the feed, so the server never scans the ring"
    );
    assert_eq!(scoped[0]["account"], format!("carol.{tla}"));

    let feed: Vec<serde_json::Value> = registry
        .view("list_recent_activity")
        .args_json(json!({ "from_index": 0, "limit": 10, "account": null }))
        .await?
        .json()?;
    assert_eq!(
        feed[0]["event"], "sub_account_rented",
        "the feed is newest first"
    );
    assert_eq!(feed[0]["account"], format!("carol.{tla}"));
    assert_eq!(
        feed.iter()
            .filter(|e| e["event"] == "sub_account_rented")
            .count(),
        2,
        "both rents must be recorded on chain"
    );

    Ok(())
}

#[tokio::test]
async fn a_wallet_with_many_co_owners_is_still_transferable() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let tenant = rent(&fleet, &registry, &tla, "alice").await?;

    fleet
        .council
        .transfer_near(&tenant, NearToken::from_near(5))
        .await?
        .into_result()?;

    let mut added = 0usize;
    for batch in 0..20 {
        let ops: Vec<serde_json::Value> = (0..25)
            .map(|i| {
                json!({
                    "op": "add_extension",
                    "payload": { "account_id": format!("pad{batch}x{i}.test.near") }
                })
            })
            .collect();
        let out = fleet
            .bob
            .call(&tenant, "w_execute_extension")
            .args_json(json!({ "request": { "internal": ops } }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await?;
        if out.into_result().is_err() {
            break;
        }
        added += 25;
    }
    println!("padded the extension set to {added} entries");

    arm_transfer(&fleet, &tenant).await?;
    let moved = fleet
        .bob
        .call(registry.id(), "nft_transfer")
        .args_json(json!({
            "receiver_id": fleet.relay.id(),
            "token_id": format!("alice.{tla}"),
            "approval_id": null,
            "memo": null,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    moved.into_result()?;
    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.relay.id().as_str(),
        "a wallet padded with {added} co-owners must still rotate"
    );
    Ok(())
}
