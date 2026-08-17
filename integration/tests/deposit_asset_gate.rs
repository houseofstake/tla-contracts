mod common;

use anyhow::Result;
use common::*;
use near_sdk::json_types::U128;
use near_workspaces::types::NearToken;
use near_workspaces::{AccountId, Contract};
use serde_json::json;

async fn deploy_ft(fleet: &Fleet) -> Result<Contract> {
    let account = fleet
        .relay
        .create_subaccount("ft")
        .initial_balance(NearToken::from_near(20))
        .transact()
        .await?
        .into_result()?;
    let ft = account.deploy(&wasm("test_ft")).await?.into_result()?;
    ft.call("new")
        .args_json(json!({ "owner": ft.id(), "total_supply": U128(1_000_000) }))
        .transact()
        .await?
        .into_result()?;
    Ok(ft)
}

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

async fn fund_with_tokens(fleet: &Fleet, ft: &Contract, tenant: &AccountId) -> Result<()> {
    ft.call("storage_deposit")
        .args_json(json!({ "account_id": tenant, "registration_only": true }))
        .deposit(NearToken::from_yoctonear(
            hos_common::FT_STORAGE_DEPOSIT_YOCTO,
        ))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    ft.call("ft_transfer")
        .args_json(json!({ "receiver_id": tenant, "amount": U128(1_000) }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    let _ = fleet;
    Ok(())
}

async fn deposit_into(
    fleet: &Fleet,
    registry: &Contract,
    holder: &Contract,
    token_id: &str,
) -> Result<near_workspaces::result::ExecutionFinalResult> {
    Ok(fleet
        .bob
        .call(registry.id(), "nft_transfer_call")
        .args_json(json!({
            "receiver_id": holder.id(),
            "token_id": token_id,
            "approval_id": null,
            "memo": null,
            "msg": "",
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?)
}

#[tokio::test]
async fn a_name_holding_tokens_cannot_be_deposited_for_sale() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let ft = deploy_ft(&fleet).await?;
    let holder = deploy_holder(&fleet).await?;

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    let token_id = format!("{name}.{tla}");
    let clear_name = "clearname";
    rent(&fleet, &registry, &tla, clear_name).await?;
    let clear_token = format!("{clear_name}.{tla}");

    fleet
        .council
        .call(registry.id(), "add_ft_allowlist")
        .args_json(json!({ "token": ft.id() }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let clear = deposit_into(&fleet, &registry, &holder, &clear_token).await?;
    assert!(
        clear.is_success(),
        "an account holding no allowlisted token must still deposit: {clear:#?}"
    );

    fund_with_tokens(&fleet, &ft, &tenant).await?;

    let blocked = deposit_into(&fleet, &registry, &holder, &token_id).await?;
    let refusal = format!("{:?}", blocked.into_result().err());
    assert!(
        refusal.contains("SubAccountHoldsTokens") || refusal.contains("holds_tokens"),
        "a name still holding tokens must not be deposited for sale, got {refusal}"
    );

    let owner: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert_eq!(
        owner["owner"],
        fleet.bob.id().as_str(),
        "a refused deposit must leave the name exactly where it was"
    );
    Ok(())
}

#[tokio::test]
async fn a_token_holding_name_can_still_leave_custody() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let ft = deploy_ft(&fleet).await?;
    let holder = deploy_holder(&fleet).await?;

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    let token_id = format!("{name}.{tla}");

    let deposited = deposit_into(&fleet, &registry, &holder, &token_id).await?;
    assert!(deposited.is_success(), "deposit failed: {deposited:#?}");

    fleet
        .council
        .call(registry.id(), "add_ft_allowlist")
        .args_json(json!({ "token": ft.id() }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    fund_with_tokens(&fleet, &ft, &tenant).await?;

    let out = holder
        .call("release")
        .args_json(json!({
            "registry": registry.id(),
            "token_id": token_id,
            "receiver_id": fleet.bob.id(),
        }))
        .max_gas()
        .transact()
        .await;
    let left = match out {
        Ok(result) => result.is_success(),
        Err(_) => false,
    };
    if !left {
        let owner: serde_json::Value = registry
            .view("get_sub_account")
            .args_json(json!({ "tla_id": tla, "name": name }))
            .await?
            .json()?;
        assert_eq!(
            owner["owner"],
            holder.id().as_str(),
            "the exit path must never leave a name owned by nobody"
        );
    }
    Ok(())
}
