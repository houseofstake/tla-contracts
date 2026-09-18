mod common;

use anyhow::Result;
use common::*;
use near_workspaces::types::NearToken;
use near_workspaces::Contract;
use serde_json::json;

async fn registrar_registry(fleet: &Fleet) -> Result<String> {
    Ok(fleet.registrar.view("registry").await?.json::<String>()?)
}

async fn keys_on(fleet: &Fleet) -> Result<usize> {
    Ok(fleet
        .worker
        .view_access_keys(fleet.registrar.id())
        .await?
        .len())
}

async fn attempt_rent(
    fleet: &Fleet,
    registry: &Contract,
    name: &str,
) -> Result<near_workspaces::result::ExecutionFinalResult> {
    let tla = fleet.registrar.id().clone();
    let total = rent_price(registry, &tla, name).await?;
    Ok(fleet
        .relay
        .call(registry.id(), "rent_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .deposit(NearToken::from_yoctonear(
            total + NearToken::from_millinear(100).as_yoctonear(),
        ))
        .max_gas()
        .transact()
        .await?)
}

async fn redeploy_registrar(fleet: &Fleet) -> Result<()> {
    fleet
        .registrar
        .as_account()
        .deploy(&wasm("registrar"))
        .await?
        .into_result()?;
    Ok(())
}

#[tokio::test]
async fn a_brick_that_writes_no_state_leaves_the_registrar_recoverable() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    rent(&fleet, &registry, &tla, "alice").await?;
    let bound_before = registrar_registry(&fleet).await?;

    fleet
        .registrar
        .as_account()
        .deploy(&wasm("mpc_recovery"))
        .await?
        .into_result()?;

    let bricked = attempt_rent(&fleet, &registry, "bob").await?;
    assert!(
        !bricked.receipt_failures().is_empty(),
        "a registrar replaced by foreign code must not mint"
    );

    redeploy_registrar(&fleet).await?;
    fleet
        .registrar
        .as_account()
        .call(fleet.registrar.id(), "migrate")
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    assert_eq!(
        registrar_registry(&fleet).await?,
        bound_before,
        "the recovered registrar must still answer to the same registry"
    );
    rent(&fleet, &registry, &tla, "carol").await?;
    Ok(())
}

#[tokio::test]
async fn a_foreign_contract_cannot_overwrite_the_registrar_state() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    rent(&fleet, &registry, &tla, "alice").await?;

    fleet
        .registrar
        .as_account()
        .deploy(&wasm("wallet_impl_deployer"))
        .await?
        .into_result()?;
    let seized = fleet
        .registrar
        .as_account()
        .call(fleet.registrar.id(), "new")
        .args_json(json!({
            "council": fleet.council.id(),
            "patch_authority": fleet.bob.id(),
            "approval_delay_ns": "0",
        }))
        .max_gas()
        .transact()
        .await?;
    assert!(
        seized.is_failure(),
        "an init on foreign code must refuse while the registrar state is still there"
    );

    redeploy_registrar(&fleet).await?;
    fleet
        .registrar
        .as_account()
        .call(fleet.registrar.id(), "migrate")
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    rent(&fleet, &registry, &tla, "carol").await?;
    Ok(())
}

#[tokio::test]
async fn the_registrar_mints_while_its_fullaccess_key_remains() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    assert_eq!(
        keys_on(&fleet).await?,
        1,
        "this test only means something while the account still holds its key"
    );
    rent(&fleet, &registry, &tla, "alice").await?;
    assert_eq!(
        keys_on(&fleet).await?,
        1,
        "minting must not strip the key the operator relies on"
    );
    Ok(())
}

#[tokio::test]
async fn no_redeploy_rebinds_the_registrar_to_another_registry() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let first = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    rent(&fleet, &first, &tla, "alice").await?;

    let second = fleet
        .worker
        .root_account()?
        .create_subaccount("registry2")
        .initial_balance(NearToken::from_near(20))
        .transact()
        .await?
        .into_result()?;

    redeploy_registrar(&fleet).await?;
    fleet
        .registrar
        .as_account()
        .call(fleet.registrar.id(), "migrate")
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    assert_eq!(
        registrar_registry(&fleet).await?,
        fleet.registry.id().to_string(),
        "migrate carries the original binding forward, so it cannot repoint the registrar"
    );

    let reinit = fleet
        .registrar
        .as_account()
        .call(fleet.registrar.id(), "new")
        .args_json(json!({ "config": {
            "registry": second.id(),
            "council": fleet.council.id(),
            "wallet_impl": fleet.impl_account,
            "hos_extension": fleet.extension.id(),
            "recovery": fleet.recovery.id(),
            "chain_id": "testnet",
            "min_balance": NearToken::from_millinear(100),
            "wallet_timeout_secs": 3600,
        }}))
        .max_gas()
        .transact()
        .await?;
    assert!(
        reinit.is_failure(),
        "new cannot rebind either, because it refuses to run over existing state"
    );
    Ok(())
}
