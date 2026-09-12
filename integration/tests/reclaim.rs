mod common;

use anyhow::{bail, Result};
use common::*;
use near_sdk::json_types::U128;
use serde_json::json;

const SHORT_TERM_NS: u64 = 60 * 1_000_000_000;
const BLOCKS_PAST_TERM_AND_GRACE: u64 = 4_000;

#[tokio::test]
async fn an_expired_lease_is_reclaimed_and_the_name_is_parked() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry_with_terms(&fleet, Some(SHORT_TERM_NS), SHORT_TERM_NS).await?;
    let tla = fleet.registrar.id().clone();

    let name = "lapsing";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.bob.id().as_str(),
        "the rent must land before the lease can be allowed to lapse"
    );

    fleet
        .worker
        .fast_forward(BLOCKS_PAST_TERM_AND_GRACE)
        .await?;

    let before: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert_eq!(
        before["lifecycle"], "Reclaimable",
        "the lease did not lapse, so the reclaim below would prove nothing"
    );

    let reclaimed = fleet
        .relay
        .call(registry.id(), "reclaim_finalize")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .max_gas()
        .transact()
        .await?;
    if let Some(failure) = reclaimed.receipt_failures().first() {
        bail!("reclaim_finalize failed: {failure:?}");
    }

    let after: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert!(
        after.is_null(),
        "a finalized reclaim must remove the sub-account, got {after}"
    );

    let parked: serde_json::Value = registry
        .view("get_parked_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert!(
        !parked.is_null(),
        "a reclaimed name must be parked rather than vanish"
    );

    let available: bool = registry
        .view("is_name_available")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert!(
        !available,
        "a parked name must not read as available to the next renter"
    );

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        "",
        "the registry's own ledger agreeing with itself proves nothing about the \
         account: the renter must be out of the wallet's extension set, or a parked \
         name is still driveable by whoever held it"
    );
    let extensions: Vec<String> = fleet.worker.view(&tenant, "w_extensions").await?.json()?;
    assert_eq!(
        extensions,
        vec![fleet.extension.id().to_string()],
        "only the lease authority may remain on a parked account"
    );
    let lease: serde_json::Value = fleet.worker.view(&tenant, "hos_lease").await?.json()?;
    assert_eq!(
        lease["state"], "Parked",
        "the wallet has to carry the park too, or it keeps serving a lease the \
         registry has already taken back"
    );

    Ok(())
}

#[tokio::test]
async fn a_paid_re_rent_keeps_the_payout_the_registry_was_told_to_use() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry_with_terms(&fleet, Some(SHORT_TERM_NS), SHORT_TERM_NS).await?;
    let tla = fleet.registrar.id().clone();

    let name = "employee";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    fleet
        .worker
        .fast_forward(BLOCKS_PAST_TERM_AND_GRACE)
        .await?;
    fleet
        .relay
        .call(registry.id(), "reclaim_finalize")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    for (method, args) in [
        (
            "add_payment_authority",
            json!({ "account_id": fleet.relay.id() }),
        ),
        (
            "bind_payment_authority_tla",
            json!({ "account_id": fleet.relay.id(), "tla_id": tla, "max_mints": "4" }),
        ),
    ] {
        fleet
            .council
            .call(registry.id(), method)
            .args_json(args)
            .deposit(near_workspaces::types::NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
    }

    let re_rented = fleet
        .relay
        .call(registry.id(), "rent_sub_account_paid")
        .args_json(json!({
            "tla_id": tla,
            "name": name,
            "owner_account": fleet.bob.id(),
            "payout_account": fleet.council.id(),
            "order_id": "ord-employee",
        }))
        .deposit(near_workspaces::types::NearToken::from_millinear(300))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = re_rented.receipt_failures().first() {
        bail!("paid re-rent receipt failed: {failure:?}");
    }

    let wallet_payout: String = fleet
        .worker
        .view(&tenant, "hos_payout_account")
        .await?
        .json()?;
    let recorded: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;

    assert_eq!(
        recorded["payout_account"],
        fleet.council.id().as_str(),
        "the registry must keep the payout it was handed, not the incoming owner"
    );
    assert_eq!(
        wallet_payout,
        recorded["payout_account"].as_str().unwrap_or_default(),
        "a sweep would pay the wrong account whenever the wallet and the registry disagree"
    );
    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.bob.id().as_str(),
        "the incoming owner must still receive the name"
    );

    let lease: serde_json::Value = fleet.worker.view(&tenant, "hos_lease").await?.json()?;
    assert_eq!(
        lease["state"], "Active",
        "settlement must not report success while the wallet is still parked"
    );
    Ok(())
}

#[tokio::test]
async fn a_re_rented_name_reaches_its_new_renter_in_working_order() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry_with_terms(&fleet, Some(SHORT_TERM_NS), SHORT_TERM_NS).await?;
    let tla = fleet.registrar.id().clone();

    let name = "secondhand";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    fleet
        .worker
        .fast_forward(BLOCKS_PAST_TERM_AND_GRACE)
        .await?;
    fleet
        .relay
        .call(registry.id(), "reclaim_finalize")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let price = rent_price(&registry, &tla, name).await?;
    let re_rented = fleet
        .relay
        .call(registry.id(), "rent_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .deposit(near_workspaces::types::NearToken::from_yoctonear(price))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = re_rented.receipt_failures().first() {
        bail!("re-rent receipt failed: {failure:?}");
    }

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.relay.id().as_str(),
        "the re-rent must hand the wallet to whoever paid for it"
    );

    let lease: serde_json::Value = fleet.worker.view(&tenant, "hos_lease").await?.json()?;
    let until: u64 = lease["lease_until_ns"].as_str().unwrap().parse()?;
    let now = chain_timestamp_ns(&fleet.worker).await?;
    assert_eq!(
        lease["state"], "Active",
        "a name sold on must not still read as parked to its own wallet"
    );
    assert!(
        until > now,
        "the new renter paid for a term the wallet never heard about: the wallet \
         still holds the previous renter's expired lease ({until} against {now}), \
         so every method they paid to use refuses them"
    );

    Ok(())
}

#[tokio::test]
async fn a_live_lease_cannot_be_reclaimed() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry_with_terms(&fleet, Some(SHORT_TERM_NS), SHORT_TERM_NS).await?;
    let tla = fleet.registrar.id().clone();

    let name = "current";
    rent(&fleet, &registry, &tla, name).await?;

    let refused = fleet
        .relay
        .call(registry.id(), "reclaim_finalize")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .max_gas()
        .transact()
        .await?;
    let landed = refused.is_success() && refused.receipt_failures().is_empty();
    assert!(
        !landed,
        "a lease inside its term must never be reclaimable, or the term means nothing"
    );

    let still: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert_eq!(
        still["owner"],
        fleet.bob.id().as_str(),
        "the refused reclaim must leave the name with its renter"
    );
    Ok(())
}

#[tokio::test]
async fn a_reclaim_through_the_asset_gate_still_parks_the_wallet() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry_with_terms(&fleet, Some(SHORT_TERM_NS), SHORT_TERM_NS).await?;
    let tla = fleet.registrar.id().clone();

    let ft = fleet
        .relay
        .create_subaccount("ft")
        .initial_balance(near_workspaces::types::NearToken::from_near(20))
        .transact()
        .await?
        .into_result()?
        .deploy(&wasm("test_ft"))
        .await?
        .into_result()?;
    ft.call("new")
        .args_json(json!({ "owner": ft.id(), "total_supply": U128(1_000_000) }))
        .transact()
        .await?
        .into_result()?;
    fleet
        .council
        .call(registry.id(), "add_ft_allowlist")
        .args_json(json!({ "token": ft.id() }))
        .deposit(near_workspaces::types::NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let name = "gated";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    fleet
        .worker
        .fast_forward(BLOCKS_PAST_TERM_AND_GRACE)
        .await?;

    let reclaimed = fleet
        .relay
        .call(registry.id(), "reclaim_finalize")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .max_gas()
        .transact()
        .await?;
    if let Some(failure) = reclaimed.receipt_failures().first() {
        bail!("reclaim through the balance fan-out failed: {failure:?}");
    }

    let parked: serde_json::Value = registry
        .view("get_parked_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert!(
        !parked.is_null(),
        "the balance gate runs the park out of a fixed callback budget, so a longer \
         park chain has to be funded there too"
    );
    let lease: serde_json::Value = fleet.worker.view(&tenant, "hos_lease").await?.json()?;
    assert_eq!(lease["state"], "Parked");
    Ok(())
}
