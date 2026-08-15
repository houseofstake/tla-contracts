mod common;

use anyhow::{bail, Result};
use common::*;
use near_workspaces::types::NearToken;
use near_workspaces::Contract;
use serde_json::json;

async fn deploy_holder(fleet: &Fleet) -> Result<Contract> {
    let account = fleet
        .relay
        .create_subaccount("holder")
        .initial_balance(NearToken::from_near(10))
        .transact()
        .await?
        .into_result()?;
    let holder = account.deploy(&wasm("test_dapp")).await?.into_result()?;
    holder
        .call("new")
        .args_json(json!({ "token": fleet.relay.id() }))
        .transact()
        .await?
        .into_result()?;
    Ok(holder)
}

#[tokio::test]
async fn a_holder_can_return_a_deposited_name() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let holder = deploy_holder(&fleet).await?;

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    let token_id = format!("{name}.{tla}");

    let deposited = fleet
        .bob
        .call(registry.id(), "nft_transfer_call")
        .args_json(json!({
            "receiver_id": holder.id(),
            "token_id": token_id,
            "approval_id": null,
            "memo": null,
            "msg": "hold",
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = deposited.receipt_failures().first() {
        bail!("deposit receipt failed: {failure:?}");
    }
    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        holder.id().as_str(),
        "the holder must actually hold the account, not merely be recorded against it"
    );

    let returned = holder
        .as_account()
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
    if let Some(failure) = returned.receipt_failures().first() {
        bail!("the holder could not return the name: {failure:?}");
    }

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.relay.id().as_str(),
        "the wallet must follow the withdrawal"
    );
    let sub: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert_eq!(
        sub["owner"],
        fleet.relay.id().as_str(),
        "and the registry index must follow it too"
    );

    Ok(())
}

#[tokio::test]
async fn a_stranger_cannot_withdraw_a_name_deposited_by_someone_else() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let holder = deploy_holder(&fleet).await?;

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    let token_id = format!("{name}.{tla}");

    fleet
        .bob
        .call(registry.id(), "nft_transfer_call")
        .args_json(json!({
            "receiver_id": holder.id(),
            "token_id": token_id,
            "approval_id": null,
            "memo": null,
            "msg": "hold",
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let stolen = fleet
        .bob
        .call(registry.id(), "nft_transfer")
        .args_json(json!({
            "receiver_id": fleet.bob.id(),
            "token_id": token_id,
            "approval_id": null,
            "memo": null,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    let landed = stolen.is_success() && stolen.receipt_failures().is_empty();
    assert!(
        !landed,
        "the previous owner must not be able to reclaim a name it deposited"
    );
    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        holder.id().as_str(),
        "the deposit must survive the attempt"
    );

    Ok(())
}
