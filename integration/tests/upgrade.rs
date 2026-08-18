mod common;

use anyhow::Result;
use common::*;
use near_sdk::json_types::{Base64VecU8, U64};
use near_workspaces::types::NearToken;
use serde_json::json;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn the_council_upgrades_a_registry_that_holds_no_key() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    rent(&fleet, &registry, &tla, "alice").await?;

    let key = fleet.registry.secret_key().public_key();
    fleet
        .registry
        .batch(fleet.registry.id())
        .delete_key(key)
        .transact()
        .await?
        .into_result()?;
    assert!(
        fleet
            .worker
            .view_access_keys(fleet.registry.id())
            .await?
            .is_empty(),
        "the registry account must hold no key for the rest of this test to mean anything"
    );

    let wrong = wasm("registrar");
    let wrong_hash = bs58::encode(Sha256::digest(&wrong)).into_string();
    fleet
        .council
        .call(registry.id(), "approve_upgrade")
        .args_json(json!({ "code_hash": wrong_hash }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    let bad = fleet
        .council
        .call(registry.id(), "upgrade")
        .args_json(json!({ "code": Base64VecU8(wrong) }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    assert!(
        !bad.receipt_failures().is_empty(),
        "code the account cannot migrate into must not land"
    );
    let still_here: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": "alice" }))
        .await?
        .json()?;
    assert_eq!(
        still_here["owner"],
        fleet.bob.id().as_str(),
        "a refused upgrade must leave the old code and its state untouched"
    );

    let code = wasm("tla_registry");
    let hash = bs58::encode(Sha256::digest(&code)).into_string();
    fleet
        .council
        .call(registry.id(), "approve_upgrade")
        .args_json(json!({ "code_hash": hash }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let too_soon = fleet
        .council
        .call(registry.id(), "upgrade")
        .args_json(json!({ "code": Base64VecU8(code) }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    let refusal = format!("{:?}", too_soon.into_result().err());
    assert!(
        refusal.contains("approval_too_young"),
        "matching code still waits out the delay, got {refusal}"
    );

    let sub: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": "alice" }))
        .await?
        .json()?;
    assert_eq!(
        sub["owner"],
        fleet.bob.id().as_str(),
        "state must survive a refused deploy"
    );
    let pending: Option<String> = registry.view("approved_upgrade_hash").await?.json()?;
    assert_eq!(
        pending.as_deref(),
        Some(hash.as_str()),
        "a refused upgrade must leave its approval standing"
    );

    Ok(())
}

#[tokio::test]
async fn a_registry_deployed_without_the_field_takes_the_full_delay() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let account = fleet
        .relay
        .create_subaccount("reg2")
        .initial_balance(NearToken::from_near(20))
        .transact()
        .await?
        .into_result()?;
    let registry = account.deploy(&wasm("tla_registry")).await?.into_result()?;
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

    let code = wasm("registrar");
    let hash = bs58::encode(Sha256::digest(&code)).into_string();
    fleet
        .council
        .call(registry.id(), "approve_upgrade")
        .args_json(json!({ "code_hash": hash }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    let too_soon = fleet
        .council
        .call(registry.id(), "upgrade")
        .args_json(json!({ "code": Base64VecU8(code) }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    let refusal = format!("{:?}", too_soon.into_result().err());
    assert!(
        refusal.contains("approval_too_young"),
        "omitting the field must leave the 48 hour delay in force, got {refusal}"
    );

    Ok(())
}

#[tokio::test]
async fn a_keyless_contract_completes_an_upgrade_and_keeps_its_state() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let deployer = fleet.deployer.clone();

    let council_before: serde_json::Value = deployer.view("config").await?.json()?;
    let hash_before: Option<String> = deployer.view("current_hash").await?.json()?;

    let key = fleet.deployer.as_account().secret_key().public_key();
    fleet
        .deployer
        .as_account()
        .batch(deployer.id())
        .delete_key(key)
        .transact()
        .await?
        .into_result()?;
    assert!(
        fleet
            .worker
            .view_access_keys(deployer.id())
            .await?
            .is_empty(),
        "the deployer must hold no key for this to prove the post-seal escape hatch"
    );

    let code = wasm("wallet_impl_deployer");
    let hash = bs58::encode(Sha256::digest(&code)).into_string();
    fleet
        .council
        .call(deployer.id(), "approve_self_upgrade")
        .args_json(json!({ "hash": hash }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let upgraded = fleet
        .council
        .call(deployer.id(), "upgrade_self")
        .args_json(json!({ "code": Base64VecU8(code) }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    upgraded.into_result()?;

    let council_after: serde_json::Value = deployer.view("config").await?.json()?;
    assert_eq!(
        council_before["council"], council_after["council"],
        "migrate must carry the council across a keyless self upgrade"
    );
    let hash_after: Option<String> = deployer.view("current_hash").await?.json()?;
    assert_eq!(
        hash_before, hash_after,
        "migrate must carry the published implementation hash across the upgrade"
    );
    assert!(
        fleet
            .worker
            .view_access_keys(deployer.id())
            .await?
            .is_empty(),
        "an upgrade must not reintroduce a key onto a sealed account"
    );

    Ok(())
}
