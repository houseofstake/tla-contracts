mod common;

use anyhow::{bail, Result};
use common::*;
use near_sdk::NearToken;
use near_workspaces::types::Gas;
use serde_json::json;

async fn payout_account_of(fleet: &Fleet, tenant: &near_workspaces::AccountId) -> Result<String> {
    Ok(fleet
        .worker
        .view(tenant, "hos_payout_account")
        .await?
        .json()?)
}

#[tokio::test]
async fn payout_account_is_pinned_at_mint() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let tenant = mint(&fleet, "pinned", NearToken::from_near(5)).await?;

    assert_eq!(
        payout_account_of(&fleet, &tenant).await?,
        fleet.bob.id().to_string(),
        "the wallet must carry the user's own account from the moment it exists"
    );
    Ok(())
}

#[tokio::test]
async fn expired_lease_sweeps_near_to_the_users_own_account() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let now = chain_timestamp_ns(&fleet.worker).await?;
    let short_lease = now + 3_000_000_000;
    let owner = fleet.bob.id().clone();
    let tenant = mint_owned_by(
        &fleet,
        "sweepme",
        &owner,
        NearToken::from_near(20),
        short_lease,
    )
    .await?;

    fleet.worker.fast_forward(200).await?;
    if chain_timestamp_ns(&fleet.worker).await? <= short_lease {
        bail!("virtual clock did not pass the lease end");
    }

    let before = balance_of(&fleet.worker, fleet.bob.id()).await?;
    let swept = fleet
        .extension
        .call(&tenant, "hos_sweep_near")
        .args_json(json!({}))
        .max_gas()
        .transact()
        .await?;
    if !swept.is_success() {
        bail!("sweep must succeed after expiry: {swept:#?}");
    }
    if let Some(failure) = swept.receipt_failures().first() {
        bail!("sweep receipt failed: {failure:?}");
    }

    let after = balance_of(&fleet.worker, fleet.bob.id()).await?;
    assert!(
        after > before,
        "the user's own account must receive the balance: {before} -> {after}"
    );

    let left = balance_of(&fleet.worker, &tenant).await?;
    assert!(
        left < NearToken::from_millinear(100).as_yoctonear(),
        "the tenant account must be left at roughly the storage floor, found {left} yocto"
    );
    Ok(())
}

#[tokio::test]
async fn sweep_is_refused_while_the_lease_is_live() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let tenant = mint(&fleet, "livelease", NearToken::from_near(20)).await?;

    let before = balance_of(&fleet.worker, &tenant).await?;
    let attempted = fleet
        .extension
        .call(&tenant, "hos_sweep_near")
        .args_json(json!({}))
        .max_gas()
        .transact()
        .await?;
    assert!(
        attempted.is_failure(),
        "a live lease must refuse the sweep: {attempted:#?}"
    );

    let after = balance_of(&fleet.worker, &tenant).await?;
    assert!(
        after >= before.saturating_sub(NearToken::from_millinear(50).as_yoctonear()),
        "a refused sweep must leave the balance in place: {before} -> {after}"
    );
    Ok(())
}

#[tokio::test]
async fn nobody_can_repoint_the_payout_account_independently() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let tenant = mint(&fleet, "nochoice", NearToken::from_near(5)).await?;

    // The payout destination is derived from ownership, so there is no method
    // to move it on its own, for the authority or anyone else.
    let attempted = fleet
        .extension
        .call(&tenant, "hos_set_payout_account")
        .args_json(json!({ "new_payout_account": fleet.relay.id() }))
        .gas(Gas::from_tgas(30))
        .transact()
        .await?;
    assert!(
        attempted.is_failure(),
        "there must be no way to redirect a user's funds: {attempted:#?}"
    );

    assert_eq!(
        payout_account_of(&fleet, &tenant).await?,
        fleet.bob.id().to_string(),
        "the destination must be unchanged"
    );
    Ok(())
}

#[tokio::test]
async fn a_transfer_moves_the_payout_with_the_owner() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let tenant = mint(&fleet, "sold", NearToken::from_near(5)).await?;
    arm_transfer(&fleet, &tenant).await?;

    let seller_before = balance_of(&fleet.worker, fleet.bob.id()).await?;
    let buyer_before = balance_of(&fleet.worker, fleet.relay.id()).await?;

    let moved = fleet
        .extension
        .call(&tenant, "hos_transfer_ownership")
        .args_json(json!({ "to": fleet.relay.id(), "cause": "Sale" }))
        .deposit(near_workspaces::types::NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(30))
        .transact()
        .await?;
    if !moved.is_success() {
        bail!("the authority must be able to transfer ownership: {moved:#?}");
    }
    if let Some(failure) = moved.receipt_failures().first() {
        bail!("transfer receipt failed: {failure:?}");
    }

    assert_eq!(
        payout_account_of(&fleet, &tenant).await?,
        fleet.relay.id().to_string(),
        "the payout must follow the new owner"
    );

    let seller_after = balance_of(&fleet.worker, fleet.bob.id()).await?;
    let buyer_after = balance_of(&fleet.worker, fleet.relay.id()).await?;
    assert!(
        seller_after > seller_before + NearToken::from_near(4).as_yoctonear(),
        "the outgoing owner must receive the balance: {seller_before} -> {seller_after}"
    );
    assert_eq!(
        buyer_after, buyer_before,
        "the incoming owner must inherit the name, never the previous owner's funds"
    );
    let left = balance_of(&fleet.worker, &tenant).await?;
    assert!(
        left < NearToken::from_millinear(100).as_yoctonear(),
        "the account must be left at roughly the storage floor, found {left} yocto"
    );
    Ok(())
}

#[tokio::test]
async fn a_recovery_leaves_the_balance_with_the_recovered_account() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let tenant = mint(&fleet, "lostkeys", NearToken::from_near(5)).await?;
    arm_transfer(&fleet, &tenant).await?;

    let before = balance_of(&fleet.worker, &tenant).await?;
    let moved = fleet
        .extension
        .call(&tenant, "hos_transfer_ownership")
        .args_json(json!({ "to": fleet.relay.id(), "cause": "Recovery" }))
        .deposit(near_workspaces::types::NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(30))
        .transact()
        .await?;
    if !moved.is_success() {
        bail!("recovery must be able to rotate ownership: {moved:#?}");
    }

    let after = balance_of(&fleet.worker, &tenant).await?;
    assert!(
        after > before - NearToken::from_millinear(100).as_yoctonear(),
        "a recovery must not move funds to an account the user has lost: {before} -> {after}"
    );
    Ok(())
}
