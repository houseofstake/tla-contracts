mod common;

use anyhow::Result;
use common::*;
use near_workspaces::types::NearToken;
use serde_json::json;
use sha2::{Digest, Sha256};

const YOCTO: NearToken = NearToken::from_yoctonear(1);

#[tokio::test]
async fn the_deployer_publishes_registrar_code_as_a_global() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let root = worker.root_account()?;

    let council = root
        .create_subaccount("council")
        .initial_balance(NearToken::from_near(60))
        .transact()
        .await?
        .into_result()?;
    let publisher = root
        .create_subaccount("rgl")
        .initial_balance(NearToken::from_near(60))
        .transact()
        .await?
        .into_result()?;

    let deployer = publisher
        .deploy(&wasm("wallet_impl_deployer"))
        .await?
        .into_result()?;
    deployer
        .call("new")
        .args_json(json!({ "council": council.id(), "approval_delay_ns": "0" }))
        .transact()
        .await?
        .into_result()?;

    let registrar_wasm = wasm("registrar");
    let registrar_hash = bs58::encode(Sha256::digest(&registrar_wasm)).into_string();

    let unapproved = council
        .call(deployer.id(), "gd_deploy")
        .args_json(json!({ "code": code_arg(&registrar_wasm) }))
        .deposit(NearToken::from_near(20))
        .max_gas()
        .transact()
        .await?;
    assert!(
        unapproved.into_result().is_err(),
        "code the council never approved must not reach the global slot"
    );

    council
        .call(deployer.id(), "gd_approve")
        .args_json(json!({ "hash": registrar_hash }))
        .deposit(YOCTO)
        .transact()
        .await?
        .into_result()?;

    let cost = NearToken::from_yoctonear(registrar_wasm.len() as u128 * GLOBAL_CODE_COST_PER_BYTE);
    council
        .call(deployer.id(), "gd_deploy")
        .args_json(json!({ "code": code_arg(&registrar_wasm) }))
        .deposit(cost)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let current: Option<String> = deployer.view("current_hash").await?.json()?;
    assert_eq!(
        current.as_deref(),
        Some(registrar_hash.as_str()),
        "current_hash is written only from the success branch of gd_on_deployed, \
         so this is the proof the global publish landed"
    );

    Ok(())
}

#[tokio::test]
async fn the_caller_deposit_carries_the_publish_not_the_publisher_balance() -> Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let root = worker.root_account()?;

    let council = root
        .create_subaccount("council")
        .initial_balance(NearToken::from_near(60))
        .transact()
        .await?
        .into_result()?;
    let publisher = root
        .create_subaccount("thin")
        .initial_balance(NearToken::from_near(3))
        .transact()
        .await?
        .into_result()?;

    let deployer = publisher
        .deploy(&wasm("wallet_impl_deployer"))
        .await?
        .into_result()?;
    deployer
        .call("new")
        .args_json(json!({ "council": council.id(), "approval_delay_ns": "0" }))
        .transact()
        .await?
        .into_result()?;

    let registrar_wasm = wasm("registrar");
    let registrar_hash = bs58::encode(Sha256::digest(&registrar_wasm)).into_string();
    let cost = NearToken::from_yoctonear(registrar_wasm.len() as u128 * GLOBAL_CODE_COST_PER_BYTE);

    let before = deployer.view_account().await?.balance;
    assert!(
        before < cost,
        "the publisher must hold less than the publish cost for this test to mean anything, \
         held {before} against {cost}"
    );

    council
        .call(deployer.id(), "gd_approve")
        .args_json(json!({ "hash": registrar_hash }))
        .deposit(YOCTO)
        .transact()
        .await?
        .into_result()?;
    council
        .call(deployer.id(), "gd_deploy")
        .args_json(json!({ "code": code_arg(&registrar_wasm) }))
        .deposit(cost)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let current: Option<String> = deployer.view("current_hash").await?.json()?;
    assert_eq!(
        current.as_deref(),
        Some(registrar_hash.as_str()),
        "a publisher holding far less than the publish cost still published, \
         so the caller's attached deposit carries it and the publisher needs only its own storage"
    );

    Ok(())
}
