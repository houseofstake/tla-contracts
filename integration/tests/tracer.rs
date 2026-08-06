mod common;

use anyhow::Result;
use common::*;
use near_workspaces::types::{AccessKey, Gas, KeyType, NearToken, SecretKey};
use serde_json::json;
use sha2::{Digest, Sha256};

#[tokio::test]
async fn tracer_mint_owner_path_rotate_patch() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let alice = mint(&fleet, "alice", NearToken::from_near(5)).await?;

    let keys = fleet.worker.view_access_keys(&alice).await?;
    assert!(
        keys.is_empty(),
        "a leased account carries no access key, found {keys:?}"
    );
    let public_key: String = fleet.worker.view(&alice, "w_public_key").await?.json()?;
    assert_eq!(public_key, "", "a leased account reports no public key");
    let lease: serde_json::Value = fleet.worker.view(&alice, "hos_lease").await?.json()?;
    assert_eq!(lease["state"], "Active");
    assert_eq!(lease["impl_version"], 3);

    let bob_before = balance_of(&fleet.worker, fleet.bob.id()).await?;
    let outcome = fleet
        .bob
        .call(&alice, "w_execute_extension")
        .args_json(json!({
            "request": spend_request(fleet.bob.id(), near_sdk::NearToken::from_near(1))
        }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(60))
        .transact()
        .await?;
    assert!(outcome.is_success(), "owner path failed: {outcome:#?}");
    assert!(
        balance_of(&fleet.worker, fleet.bob.id()).await? > bob_before,
        "the owner transfer did not land"
    );

    let forged = fleet
        .relay
        .call(&alice, "w_execute_extension")
        .args_json(json!({
            "request": spend_request(fleet.relay.id(), near_sdk::NearToken::from_near(1))
        }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(60))
        .transact()
        .await;
    assert!(
        forged.is_err() || !forged.unwrap().is_success(),
        "a non-extension account must not drive the lease"
    );

    let seize = fleet
        .bob
        .batch(&alice)
        .add_key(
            SecretKey::from_seed(KeyType::ED25519, "seize").public_key(),
            AccessKey::full_access(),
        )
        .transact()
        .await;
    assert!(
        seize.is_err() || !seize.unwrap().is_success(),
        "nobody may bolt a key onto a leased account"
    );

    arm_transfer(&fleet, &alice).await?;
    let outcome = fleet
        .extension
        .call(&alice, "hos_transfer_ownership")
        .args_json(json!({ "to": fleet.relay.id() }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    assert!(outcome.is_success(), "transfer failed: {outcome:#?}");

    let extensions: Vec<String> = fleet.worker.view(&alice, "w_extensions").await?.json()?;
    assert!(
        extensions.iter().any(|e| e == fleet.relay.id().as_str()),
        "the new owner must hold the extension, got {extensions:?}"
    );
    assert!(
        !extensions.iter().any(|e| e == fleet.bob.id().as_str()),
        "the previous owner must be evicted, got {extensions:?}"
    );

    let stale = fleet
        .bob
        .call(&alice, "w_execute_extension")
        .args_json(json!({
            "request": spend_request(fleet.bob.id(), near_sdk::NearToken::from_yoctonear(1))
        }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(60))
        .transact()
        .await;
    assert!(
        stale.is_err() || !stale.unwrap().is_success(),
        "the old owner must lose all authority at rotation"
    );

    let wallet_wasm = wasm("hos_wallet");
    let wallet_hash = bs58::encode(Sha256::digest(&wallet_wasm)).into_string();
    fleet
        .council
        .call(fleet.deployer.id(), "gd_approve")
        .args_json(json!({ "hash": wallet_hash }))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await?
        .into_result()?;
    let publish_cost =
        NearToken::from_yoctonear(wallet_wasm.len() as u128 * GLOBAL_CODE_COST_PER_BYTE);
    let outcome = fleet
        .relay
        .call(fleet.deployer.id(), "gd_deploy")
        .args_json(json!({ "code": wallet_wasm }))
        .deposit(publish_cost)
        .max_gas()
        .transact()
        .await?;
    assert!(outcome.is_success(), "re-publish failed: {outcome:#?}");

    let lease: serde_json::Value = fleet.worker.view(&alice, "hos_lease").await?.json()?;
    assert_eq!(
        lease["state"], "Active",
        "minted account must keep working after fleet patch"
    );

    Ok(())
}

#[tokio::test]
async fn short_labels_and_foreign_minters_rejected() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let outcome = fleet
        .registry
        .call(fleet.registrar.id(), "create_sub_account")
        .args_json(json!({
            "name": "ab",
            "owner_account": fleet.bob.id(),
            "payout_account": fleet.bob.id(),
            "lease_until_ns": lease_until_ns(),
        }))
        .deposit(NearToken::from_millinear(500))
        .max_gas()
        .transact()
        .await?;
    assert!(!outcome.is_success(), "2-char label must be rejected");

    let outcome = fleet
        .council
        .call(fleet.registrar.id(), "create_sub_account")
        .args_json(json!({
            "name": "mallory",
            "owner_account": fleet.bob.id(),
            "payout_account": fleet.bob.id(),
            "lease_until_ns": lease_until_ns(),
        }))
        .deposit(NearToken::from_millinear(500))
        .max_gas()
        .transact()
        .await?;
    assert!(
        !outcome.is_success(),
        "non-registry caller must not mint, even council"
    );
    Ok(())
}

#[tokio::test]
async fn unapproved_code_cannot_publish() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let rogue = vec![7u8; 64];
    let outcome = fleet
        .relay
        .call(fleet.deployer.id(), "gd_deploy")
        .args_json(json!({ "code": rogue }))
        .deposit(NearToken::from_near(1))
        .max_gas()
        .transact()
        .await?;
    assert!(!outcome.is_success(), "unapproved code must not publish");
    Ok(())
}
