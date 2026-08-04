mod common;

use anyhow::Result;
use common::*;
use defuse_wallet::{NearPromise, Request};
use near_workspaces::network::Sandbox;
use near_workspaces::types::{AccessKey, Gas, KeyType, NearToken, SecretKey};
use near_workspaces::{Account, AccountId, Worker};
use serde_json::json;

fn assert_defended(
    result: Result<near_workspaces::result::ExecutionFinalResult, near_workspaces::error::Error>,
    what: &str,
) {
    match result {
        Err(_) => {}
        Ok(outcome) => assert!(
            outcome.is_failure(),
            "SEIZED via {what}: transaction succeeded on chain\n{outcome:#?}"
        ),
    }
}

async fn extensions_of(worker: &Worker<Sandbox>, id: &AccountId) -> Result<Vec<String>> {
    Ok(worker.view(id, "w_extensions").await?.json()?)
}

fn sdk_id(id: &AccountId) -> near_sdk::AccountId {
    id.as_str().parse().unwrap()
}

fn transfer_to(id: &AccountId, amount: near_sdk::NearToken) -> NearPromise {
    NearPromise::new(sdk_id(id)).transfer(amount)
}

/// The renter is an ordinary account listed as an extension. It never holds a
/// key on the leased account, so escalation has to go through the wallet.
async fn renter_of(fleet: &Fleet) -> &Account {
    &fleet.bob
}

async fn spend(fleet: &Fleet, alice: &AccountId, to: &AccountId, amount: NearToken) -> Result<()> {
    let promise = transfer_to(
        to,
        near_sdk::NearToken::from_yoctonear(amount.as_yoctonear()),
    );
    let outcome = renter_of(fleet)
        .await
        .call(alice, "w_execute_extension")
        .args_json(json!({ "request": Request::new().external([promise]) }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(60))
        .transact()
        .await?;
    assert!(outcome.is_success(), "legit spend must work: {outcome:#?}");
    Ok(())
}

#[tokio::test]
async fn a_leased_account_carries_no_key_to_steal() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let alice = mint(&fleet, "alice", NearToken::from_near(20)).await?;

    assert!(
        fleet.worker.view_access_keys(&alice).await?.is_empty(),
        "a leased account must carry no access key at all"
    );

    // The seed still drives the account, but only by signing a request that a
    // relayer submits, so there is nothing on-chain for an intruder to reuse.
    spend(&fleet, &alice, fleet.relay.id(), NearToken::from_near(1)).await?;
    Ok(())
}

#[tokio::test]
async fn an_intruder_cannot_attach_a_key_to_the_account() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let alice = mint(&fleet, "alice", NearToken::from_near(20)).await?;
    let intruder_key = SecretKey::from_seed(KeyType::ED25519, "intruder").public_key();

    assert_defended(
        fleet
            .bob
            .batch(&alice)
            .add_key(intruder_key.clone(), AccessKey::full_access())
            .transact()
            .await,
        "AddKey(FullAccess) from the renter",
    );
    assert_defended(
        fleet
            .relay
            .batch(&alice)
            .add_key(intruder_key, AccessKey::full_access())
            .transact()
            .await,
        "AddKey(FullAccess) from a stranger",
    );
    assert!(
        fleet.worker.view_access_keys(&alice).await?.is_empty(),
        "the account must still carry no key"
    );
    Ok(())
}

#[tokio::test]
async fn the_renter_cannot_reach_the_authority_only_methods() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let alice = mint(&fleet, "alice", NearToken::from_near(20)).await?;
    let renter = renter_of(&fleet).await;
    let far_future = chain_timestamp_ns(&fleet.worker).await? + YEAR_NS * 10;

    assert_defended(
        renter
            .call(&alice, "hos_set_lease")
            .args_json(json!({ "lease_until_ns": far_future.to_string(), "state": "Active" }))
            .deposit(NearToken::from_yoctonear(1))
            .gas(Gas::from_tgas(30))
            .transact()
            .await,
        "direct hos_set_lease",
    );

    assert_defended(
        renter
            .call(&alice, "hos_transfer_ownership")
            .args_json(json!({ "to": fleet.relay.id() }))
            .deposit(NearToken::from_yoctonear(1))
            .gas(Gas::from_tgas(30))
            .transact()
            .await,
        "direct hos_transfer_ownership",
    );

    assert_defended(
        renter
            .call(&alice, "hos_sweep_near")
            .args_json(json!({}))
            .deposit(NearToken::from_yoctonear(1))
            .gas(Gas::from_tgas(30))
            .transact()
            .await,
        "direct hos_sweep_near",
    );

    assert_defended(
        renter
            .call(&alice, "hos_sweep_ft")
            .args_json(json!({ "ft": fleet.bob.id(), "amount": "1" }))
            .deposit(NearToken::from_yoctonear(1))
            .gas(Gas::from_tgas(30))
            .transact()
            .await,
        "direct hos_sweep_ft",
    );

    assert_defended(
        renter
            .call(&alice, "hos_init")
            .args_json(json!({ "config": {
                "owner_account": fleet.bob.id(),
                "public_key": null,
                "authority": fleet.bob.id(),
                "payout_account": fleet.bob.id(),
                "lease_until_ns": far_future.to_string(),
                "timeout_secs": 3600,
            }}))
            .gas(Gas::from_tgas(30))
            .transact()
            .await,
        "re-init hos_init",
    );
    Ok(())
}

#[tokio::test]
async fn the_renter_cannot_evict_the_authority_extension() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let alice = mint(&fleet, "alice", NearToken::from_near(20)).await?;

    assert_defended(
        fleet
            .bob
            .call(&alice, "w_execute_extension")
            .args_json(json!({ "request": {
                "internal": [{ "op": "remove_extension", "payload": { "account_id": fleet.extension.id() } }]
            }}))
            .deposit(NearToken::from_yoctonear(1))
            .gas(Gas::from_tgas(40))
            .transact()
            .await,
        "RemoveExtension(authority)",
    );

    let extensions = extensions_of(&fleet.worker, &alice).await?;
    assert!(
        extensions
            .iter()
            .any(|e| e == fleet.extension.id().as_str()),
        "the authority must still hold its extension, got {extensions:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_stranger_cannot_drive_the_account_as_an_extension() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let alice = mint(&fleet, "alice", NearToken::from_near(20)).await?;

    let promise = transfer_to(fleet.relay.id(), near_sdk::NearToken::from_near(1));

    assert_defended(
        fleet
            .relay
            .call(&alice, "w_execute_extension")
            .args_json(json!({ "request": Request::new().external([promise]) }))
            .deposit(NearToken::from_yoctonear(1))
            .gas(Gas::from_tgas(60))
            .transact()
            .await,
        "w_execute_extension from a non-extension",
    );
    Ok(())
}

#[tokio::test]
async fn the_renter_cannot_make_the_wallet_call_itself() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let alice = mint(&fleet, "alice", NearToken::from_near(20)).await?;

    let self_call = transfer_to(&alice, near_sdk::NearToken::from_yoctonear(1));

    assert_defended(
        fleet
            .bob
            .call(&alice, "w_execute_extension")
            .args_json(json!({ "request": Request::new().external([self_call]) }))
            .deposit(NearToken::from_yoctonear(1))
            .gas(Gas::from_tgas(60))
            .transact()
            .await,
        "self-targeted external promise",
    );
    Ok(())
}

#[tokio::test]
async fn an_authority_freeze_stops_the_renter_and_only_the_authority_lifts_it() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let alice = mint(&fleet, "alice", NearToken::from_near(20)).await?;

    fleet
        .extension
        .call(&alice, "hos_freeze")
        .args_json(json!({}))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(30))
        .transact()
        .await?
        .into_result()?;

    let promise = transfer_to(fleet.relay.id(), near_sdk::NearToken::from_near(1));
    assert_defended(
        fleet
            .bob
            .call(&alice, "w_execute_extension")
            .args_json(json!({ "request": Request::new().external([promise]) }))
            .deposit(NearToken::from_yoctonear(1))
            .gas(Gas::from_tgas(60))
            .transact()
            .await,
        "spending while frozen",
    );

    assert_defended(
        fleet
            .bob
            .call(&alice, "hos_unfreeze")
            .args_json(json!({}))
            .deposit(NearToken::from_yoctonear(1))
            .gas(Gas::from_tgas(30))
            .transact()
            .await,
        "the renter lifting an authority freeze",
    );

    fleet
        .extension
        .call(&alice, "hos_unfreeze")
        .args_json(json!({}))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(30))
        .transact()
        .await?
        .into_result()?;

    spend(&fleet, &alice, fleet.relay.id(), NearToken::from_near(1)).await?;
    Ok(())
}
