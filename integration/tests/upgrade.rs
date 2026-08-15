mod common;

use anyhow::{bail, Result};
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

    let shipped = fleet
        .council
        .call(registry.id(), "upgrade")
        .args_json(json!({ "code": Base64VecU8(code) }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = shipped.receipt_failures().first() {
        bail!("keyless upgrade receipt failed: {failure:?}");
    }

    let sub: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": "alice" }))
        .await?
        .json()?;
    assert_eq!(
        sub["owner"],
        fleet.bob.id().as_str(),
        "state must survive a deploy that nothing signed for the account"
    );
    let spent: Option<String> = registry.view("approved_upgrade_hash").await?.json()?;
    assert!(spent.is_none(), "an approval must not survive its upgrade");

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
