mod common;

use anyhow::{bail, Result};
use common::*;
use near_sdk::json_types::U128;
use near_workspaces::types::NearToken;
use near_workspaces::{AccountId, Contract};
use serde_json::json;

const SHORT_TERM_NS: u64 = 60 * 1_000_000_000;
const PAST_TERM: u64 = 4_000;

async fn ft_under(fleet: &Fleet, label: &str) -> Result<Contract> {
    let ft = fleet
        .relay
        .create_subaccount(label)
        .initial_balance(NearToken::from_near(20))
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
    Ok(ft)
}

async fn airdrop(ft: &Contract, holder: &AccountId, amount: u128) -> Result<()> {
    ft.call("storage_deposit")
        .args_json(json!({ "account_id": holder, "registration_only": true }))
        .deposit(NearToken::from_yoctonear(
            hos_common::FT_STORAGE_DEPOSIT_YOCTO,
        ))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    ft.call("ft_transfer")
        .args_json(json!({ "receiver_id": holder, "amount": U128(amount) }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    Ok(())
}

async fn ft_of(ft: &Contract, holder: &AccountId) -> Result<u128> {
    let raw: U128 = ft
        .view("ft_balance_of")
        .args_json(json!({ "account_id": holder }))
        .await?
        .json()?;
    Ok(raw.0)
}

async fn allowlist(fleet: &Fleet, registry: &Contract, ft: &AccountId) -> Result<()> {
    fleet
        .council
        .call(registry.id(), "add_ft_allowlist")
        .args_json(json!({ "token": ft }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    Ok(())
}

async fn parked(registry: &Contract, tla: &AccountId, name: &str) -> Result<bool> {
    let value: serde_json::Value = registry
        .view("get_parked_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    Ok(!value.is_null())
}

async fn payout_of(fleet: &Fleet, tenant: &AccountId) -> Result<AccountId> {
    let payout: String = fleet
        .worker
        .view(tenant, "hos_payout_account")
        .await?
        .json()?;
    Ok(payout.parse()?)
}

async fn finalize(fleet: &Fleet, registry: &Contract, tla: &AccountId, name: &str) -> Result<bool> {
    let out = fleet
        .relay
        .call(registry.id(), "reclaim_finalize")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .max_gas()
        .transact()
        .await?;
    Ok(out.is_success())
}

async fn sweep_ft(
    fleet: &Fleet,
    registry: &Contract,
    tla: &AccountId,
    name: &str,
    ft: &AccountId,
) -> Result<bool> {
    let out = fleet
        .relay
        .call(registry.id(), "reclaim_sweep_ft")
        .args_json(json!({ "tla_id": tla, "name": name, "ft": ft }))
        .deposit(NearToken::from_yoctonear(
            hos_common::FT_STORAGE_DEPOSIT_YOCTO + 1,
        ))
        .max_gas()
        .transact()
        .await?;
    Ok(out.is_success() && out.receipt_failures().is_empty())
}

#[tokio::test]
async fn a_finalize_returns_the_native_balance_without_any_sweep_call() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry_with_terms(&fleet, Some(SHORT_TERM_NS), SHORT_TERM_NS).await?;
    let tla = fleet.registrar.id().clone();

    let name = "funded";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    fleet
        .relay
        .transfer_near(&tenant, NearToken::from_near(7))
        .await?
        .into_result()?;
    fleet.worker.fast_forward(PAST_TERM).await?;

    let payout = payout_of(&fleet, &tenant).await?;
    let payout_before = balance_of(&fleet.worker, &payout).await?;
    if !finalize(&fleet, &registry, &tla, name).await? {
        bail!("reclaim_finalize failed");
    }

    let gained = balance_of(&fleet.worker, &payout)
        .await?
        .saturating_sub(payout_before);
    assert!(
        gained > NearToken::from_near(7).as_yoctonear(),
        "the rotation sweeps the account to the outgoing payout, so a reclaim needs no \
         separate native sweep: expected over 7 NEAR, got {gained} yocto"
    );
    assert!(
        balance_of(&fleet.worker, &tenant).await? < NearToken::from_millinear(10).as_yoctonear(),
        "the account should be left at its storage reserve"
    );
    assert!(parked(&registry, &tla, name).await?);
    Ok(())
}

#[tokio::test]
async fn one_unit_of_a_listed_token_from_a_stranger_jams_the_name_until_it_is_swept() -> Result<()>
{
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry_with_terms(&fleet, Some(SHORT_TERM_NS), SHORT_TERM_NS).await?;
    let tla = fleet.registrar.id().clone();

    let ft = ft_under(&fleet, "listed").await?;
    allowlist(&fleet, &registry, ft.id()).await?;

    let name = "jammed";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    airdrop(&ft, &tenant, 1).await?;
    fleet.worker.fast_forward(PAST_TERM).await?;

    finalize(&fleet, &registry, &tla, name).await?;
    assert!(
        !parked(&registry, &tla, name).await?,
        "one unit anybody can send must not be able to take the name back"
    );

    let price = rent_price(&registry, &tla, name).await?;
    let re_rent = fleet
        .relay
        .call(registry.id(), "rent_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .deposit(NearToken::from_yoctonear(price))
        .max_gas()
        .transact()
        .await?;
    assert!(
        !re_rent.is_success(),
        "a jammed name is out of the market entirely, not merely unreclaimable"
    );

    let payout = payout_of(&fleet, &tenant).await?;
    let before = ft_of(&ft, &payout).await?;
    assert!(sweep_ft(&fleet, &registry, &tla, name, ft.id()).await?);
    assert_eq!(ft_of(&ft, &tenant).await?, 0);
    assert_eq!(
        ft_of(&ft, &payout).await?.saturating_sub(before),
        1,
        "the swept unit is paid to the holder losing the name, so jamming costs the \
         attacker and pays their target"
    );

    finalize(&fleet, &registry, &tla, name).await?;
    assert!(
        parked(&registry, &tla, name).await?,
        "the sweep is the only thing that clears the jam"
    );
    Ok(())
}

#[tokio::test]
async fn listing_a_token_after_the_lease_ended_still_returns_it_to_the_holder() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry_with_terms(&fleet, Some(SHORT_TERM_NS), SHORT_TERM_NS).await?;
    let tla = fleet.registrar.id().clone();

    let ft = ft_under(&fleet, "late").await?;
    let name = "unlisted";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    airdrop(&ft, &tenant, 500).await?;
    fleet.worker.fast_forward(PAST_TERM).await?;

    assert!(
        !sweep_ft(&fleet, &registry, &tla, name, ft.id()).await?,
        "a token nobody listed cannot be swept, which is what makes listing it the remedy"
    );

    allowlist(&fleet, &registry, ft.id()).await?;
    let payout = payout_of(&fleet, &tenant).await?;
    let before = ft_of(&ft, &payout).await?;
    assert!(sweep_ft(&fleet, &registry, &tla, name, ft.id()).await?);
    assert_eq!(ft_of(&ft, &tenant).await?, 0);
    assert_eq!(
        ft_of(&ft, &payout).await?.saturating_sub(before),
        500,
        "a holder locked out at expiry is still made whole once the token is listed"
    );
    Ok(())
}

#[tokio::test]
async fn a_parked_name_sweeps_to_the_holder_it_was_taken_from() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry_with_terms(&fleet, Some(SHORT_TERM_NS), SHORT_TERM_NS).await?;
    let tla = fleet.registrar.id().clone();

    let ft = ft_under(&fleet, "after").await?;
    allowlist(&fleet, &registry, ft.id()).await?;

    let name = "resting";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    let holder = payout_of(&fleet, &tenant).await?;
    fleet.worker.fast_forward(PAST_TERM).await?;
    if !finalize(&fleet, &registry, &tla, name).await? {
        bail!("reclaim_finalize failed");
    }
    assert!(parked(&registry, &tla, name).await?);

    airdrop(&ft, &tenant, 300).await?;
    let held_before = ft_of(&ft, &holder).await?;
    let treasury_before = ft_of(&ft, fleet.council.id()).await?;
    assert!(sweep_ft(&fleet, &registry, &tla, name, ft.id()).await?);

    assert_eq!(
        ft_of(&ft, &holder).await?.saturating_sub(held_before),
        300,
        "the wallet pays its own payout account, so parking does not redirect a sweep \
         to the treasury the registry names"
    );
    assert_eq!(
        ft_of(&ft, fleet.council.id())
            .await?
            .saturating_sub(treasury_before),
        0
    );
    Ok(())
}
