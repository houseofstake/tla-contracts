mod common;

use anyhow::Result;
use common::*;
use near_sdk::json_types::U128;
use serde_json::json;

const BLOCKS_PAST_COOLDOWN: u64 = 1_500;
const RATE_FLOOR_MICRO: u128 = 100_000;
const RATE_CEILING_MICRO: u128 = 100_000_000;

async fn set_rate(
    fleet: &Fleet,
    registry: &near_workspaces::Contract,
    micro: u128,
) -> Result<bool> {
    let out = fleet
        .council
        .call(registry.id(), "set_near_usd_rate")
        .args_json(json!({ "rate": U128(micro) }))
        .max_gas()
        .transact()
        .await?;
    Ok(out.is_success() && out.receipt_failures().is_empty())
}

async fn quote(
    registry: &near_workspaces::Contract,
    tla: &near_workspaces::AccountId,
    name: &str,
) -> Result<(u128, u128)> {
    let view: serde_json::Value = registry
        .view("get_rent_price")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    Ok((
        view["rent_yocto"].as_str().unwrap().parse()?,
        view["creation_deposit_yocto"].as_str().unwrap().parse()?,
    ))
}

#[tokio::test]
async fn a_stronger_near_buys_the_same_fee_with_fewer_yocto() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    let (rent_before, deposit_before) = quote(&registry, &tla, "priced").await?;
    fleet.worker.fast_forward(BLOCKS_PAST_COOLDOWN).await?;
    assert!(
        set_rate(&fleet, &registry, 6_000_000).await?,
        "a move inside the per-update bound must be accepted"
    );

    let (rent_after, deposit_after) = quote(&registry, &tla, "priced").await?;
    assert_eq!(
        rent_after,
        rent_before * 5 / 6,
        "rent is quoted in USD, so a 20 percent stronger NEAR must cost \
         proportionally fewer yocto, not a rounded guess at it"
    );
    assert_eq!(
        deposit_after, deposit_before,
        "the account creation deposit buys storage, not dollars, so the oracle \
         must not move it"
    );
    Ok(())
}

#[tokio::test]
async fn a_rate_that_moves_too_far_in_one_update_is_refused() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    let before = rent_price(&registry, &tla, "priced").await?;
    fleet.worker.fast_forward(BLOCKS_PAST_COOLDOWN).await?;
    assert!(
        !set_rate(&fleet, &registry, 25_000_000).await?,
        "a compromised oracle must not be able to reprice the whole catalogue in \
         one update"
    );
    assert_eq!(
        rent_price(&registry, &tla, "priced").await?,
        before,
        "a refused update must leave the previous rate in force"
    );

    assert!(
        !set_rate(&fleet, &registry, RATE_FLOOR_MICRO - 1).await?,
        "a rate under the floor must be refused even if the step were small enough"
    );
    assert!(
        !set_rate(&fleet, &registry, RATE_CEILING_MICRO + 1).await?,
        "a rate over the ceiling must be refused even if the step were small enough"
    );
    Ok(())
}

#[tokio::test]
async fn only_the_named_oracle_can_move_the_rate() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();

    let before = rent_price(&registry, &tla, "priced").await?;
    fleet.worker.fast_forward(BLOCKS_PAST_COOLDOWN).await?;
    let out = fleet
        .relay
        .call(registry.id(), "set_near_usd_rate")
        .args_json(json!({ "rate": U128(6_000_000) }))
        .max_gas()
        .transact()
        .await?;
    assert!(
        !(out.is_success() && out.receipt_failures().is_empty()),
        "anyone able to post a rate can move every price in the registry"
    );
    assert_eq!(rent_price(&registry, &tla, "priced").await?, before);
    Ok(())
}

#[tokio::test]
async fn a_second_update_inside_the_cooldown_is_refused() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;

    fleet.worker.fast_forward(BLOCKS_PAST_COOLDOWN).await?;
    assert!(set_rate(&fleet, &registry, 6_000_000).await?);
    assert!(
        !set_rate(&fleet, &registry, 5_500_000).await?,
        "the cooldown is what bounds how fast a captured oracle can walk the rate \
         somewhere the per-update bound alone would allow"
    );
    Ok(())
}
