mod common;

use anyhow::Result;
use common::*;
use near_sdk::json_types::{U128, U64};
use near_workspaces::types::NearToken;
use near_workspaces::{Account, Contract};
use serde_json::json;

fn v1_wasm(name: &str) -> Vec<u8> {
    let path = format!("fixtures/v1/{name}.wasm");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

async fn migrate_in_place(account: &Account, contract: &Contract, name: &str) -> Result<()> {
    account.deploy(&wasm(name)).await?.into_result()?;
    account
        .call(contract.id(), "migrate")
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    Ok(())
}

#[tokio::test]
async fn the_registry_carries_its_ledger_across_the_version_bump() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = fleet
        .registry
        .deploy(&v1_wasm("tla_registry"))
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
            "lease_term_ns": None::<U64>,
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    fleet
        .council
        .call(registry.id(), "admin_set_initial_rate")
        .args_json(json!({ "rate": U128(NEAR_USD_MICRO) }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    let tla = fleet.registrar.id().clone();
    fleet
        .council
        .call(registry.id(), "register_tla")
        .args_json(json!({
            "tla_id": tla,
            "tla_type": "Open",
            "premium_category": "Standard",
            "licensee": None::<String>,
        }))
        .deposit(NearToken::from_yoctonear(1))
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

    let before: serde_json::Value = registry.view("get_stats").await?.json()?;
    migrate_in_place(&fleet.registry, &registry, "tla_registry").await?;
    let after: serde_json::Value = registry.view("get_stats").await?.json()?;
    assert_eq!(
        before, after,
        "the ledger must read identically either side of the migration"
    );

    let view: serde_json::Value = registry
        .view("get_tla")
        .args_json(json!({ "tla_id": tla }))
        .await?
        .json()?;
    assert_eq!(view["lifecycle"], "Active", "the open TLA must stay open");
    assert_eq!(
        view["premium_category"], "Standard",
        "its tier must survive"
    );

    let cap: u32 = registry.view("max_tla_batch").await?.json()?;
    assert!(
        cap > 0,
        "the new code must be live after the migration, not the old code still answering"
    );
    Ok(())
}

#[tokio::test]
async fn the_extension_keeps_its_wiring_across_the_version_bump() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let extension = fleet
        .extension
        .deploy(&v1_wasm("hos_extension"))
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

    let before: String = extension.view("get_council").await?.json()?;
    migrate_in_place(&fleet.extension, &extension, "hos_extension").await?;
    let after: String = extension.view("get_council").await?.json()?;
    assert_eq!(
        before, after,
        "the council is the upgrade authority and must not move across a migration"
    );
    assert_eq!(
        after,
        fleet.council.id().to_string(),
        "and it must still be the account the contract was deployed with"
    );
    Ok(())
}

#[tokio::test]
async fn the_registrar_keeps_its_registry_pointer_across_the_version_bump() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let root = fleet.worker.root_account()?;
    let account = root
        .create_subaccount("tla2")
        .initial_balance(NearToken::from_near(10))
        .transact()
        .await?
        .into_result()?;
    let registrar = account.deploy(&v1_wasm("registrar")).await?.into_result()?;
    registrar
        .call("new")
        .args_json(json!({ "config": {
            "registry": fleet.registry.id(),
            "council": fleet.council.id(),
            "wallet_impl": fleet.impl_account,
            "hos_extension": fleet.extension.id(),
            "recovery": fleet.recovery.id(),
            "chain_id": "testnet",
            "min_balance": NearToken::from_millinear(100),
            "min_label_len": 2,
            "wallet_timeout_secs": 3600,
        }}))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let before: String = registrar.view("registry").await?.json()?;
    migrate_in_place(&account, &registrar, "registrar").await?;
    let after: String = registrar.view("registry").await?.json()?;
    assert_eq!(
        before, after,
        "only the registry may drive minting, so that pointer must not move under a \
         migration that also drops min_label_len"
    );
    assert_eq!(after, fleet.registry.id().to_string());
    Ok(())
}

#[tokio::test]
async fn the_deployer_drops_a_standing_approval_across_the_version_bump() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let root = fleet.worker.root_account()?;
    let account = root
        .create_subaccount("gd")
        .initial_balance(NearToken::from_near(10))
        .transact()
        .await?
        .into_result()?;
    let deployer = account
        .deploy(&v1_wasm("wallet_impl_deployer"))
        .await?
        .into_result()?;
    deployer
        .call("new")
        .args_json(json!({
            "council": fleet.council.id(),
            "patch_authority": fleet.bob.id(),
            "approval_delay_ns": "0",
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let pinned = "11111111111111111111111111111111";
    fleet
        .council
        .call(deployer.id(), "gd_approve")
        .args_json(json!({ "hash": pinned }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let approved: Option<String> = deployer.view("approved_hash").await?.json()?;
    assert_eq!(
        approved.as_deref(),
        Some(pinned),
        "the approval must land first"
    );

    migrate_in_place(&account, &deployer, "wallet_impl_deployer").await?;

    let standing: Option<String> = deployer.view("approved_hash").await?.json()?;
    assert!(
        standing.is_none(),
        "migrate clears a standing approval on purpose: it was given against the old \
         code, and new code must not inherit permission to publish"
    );

    let refused = fleet
        .bob
        .call(deployer.id(), "gd_approve")
        .args_json(json!({ "hash": pinned }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    assert!(
        !refused.receipt_failures().is_empty(),
        "the council field must still gate approval after the migration"
    );
    fleet
        .council
        .call(deployer.id(), "gd_approve")
        .args_json(json!({ "hash": pinned }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    Ok(())
}
