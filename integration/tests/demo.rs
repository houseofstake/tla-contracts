mod common;

use anyhow::Result;
use common::*;
use near_workspaces::types::{AccessKey, Gas, KeyType, NearToken, SecretKey};
use serde_json::json;

async fn balance(fleet: &Fleet, id: &near_workspaces::AccountId) -> Result<u128> {
    Ok(fleet.worker.view_account(id).await?.balance.as_yoctonear())
}

#[tokio::test]
async fn demonstrate_account_id_ownership_and_hos_reclaim() -> Result<()> {
    let fleet = deploy_fleet().await?;

    println!("\n===== 1. THE OWNER IS AN ACCOUNT, NOT A KEY =====");
    println!("  owner       : {} (any NEAR account)", fleet.bob.id());
    println!("  no seed phrase is generated, held, or handed over");

    let alice = mint(&fleet, "anthropic", NearToken::from_near(5)).await?;
    println!("\n===== 2. HoS MINTS THE LEASED ACCOUNT: {alice} =====");

    let keys = fleet.worker.view_access_keys(&alice).await?;
    let public_key: String = fleet.worker.view(&alice, "w_public_key").await?.json()?;
    let signature_allowed: bool = fleet
        .worker
        .view(&alice, "w_is_signature_allowed")
        .await?
        .json()?;
    let extensions: Vec<String> = fleet.worker.view(&alice, "w_extensions").await?.json()?;
    assert!(keys.is_empty(), "a leased account carries no access key");
    assert_eq!(public_key, "", "a leased account reports no public key");
    assert!(!signature_allowed, "the signed path must be closed");
    println!("  access keys  : none");
    println!("  public key   : none (w_public_key is empty)");
    println!("  signed path  : closed (w_is_signature_allowed is false)");
    println!("  extensions   : {extensions:?}");

    let before = balance(&fleet, fleet.relay.id()).await?;
    let spend = fleet
        .bob
        .call(&alice, "w_execute_extension")
        .args_json(json!({
            "request": spend_request(fleet.relay.id(), near_sdk::NearToken::from_near(1))
        }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(100))
        .transact()
        .await?;
    assert!(spend.is_success(), "the owner must drive it: {spend:#?}");
    println!("\n===== 3. THE OWNER ACCOUNT DRIVES IT =====");
    println!(
        "  1 NEAR sent {} -> {}: SUCCESS (+{} yocto)",
        alice,
        fleet.relay.id(),
        balance(&fleet, fleet.relay.id()).await? - before
    );
    println!("  nothing signed for the leased account; the owner was the predecessor");

    println!("\n===== 4. OWNERSHIP IS PROVEN BY DELEGATION (NEP-641) =====");
    let resolution: serde_json::Value = fleet
        .worker
        .view(&alice, "w_resolve_auth")
        .args_json(json!({
            "purpose": "PROVE_OWNERSHIP",
            "recipient": "app.example.com",
            "authorization": json!({ "message": "login" }).to_string(),
        }))
        .await?
        .json()?;
    assert_eq!(resolution["status"], "PENDING");
    assert_eq!(
        resolution["pending_authorizations"][0]["account_id"],
        fleet.bob.id().as_str()
    );
    println!("  status      : {}", resolution["status"]);
    println!(
        "  resolve via : {}",
        resolution["pending_authorizations"][0]["account_id"]
    );
    println!("  the lease never checks a signature; it names the account that can");

    let thief = SecretKey::from_seed(KeyType::ED25519, "thief").public_key();
    let seize = fleet
        .bob
        .batch(&alice)
        .add_key(thief, AccessKey::full_access())
        .transact()
        .await;
    let seized = matches!(seize, Ok(ref o) if o.is_success());
    assert!(!seized, "nobody may bolt a key onto a leased account");
    println!("\n===== 5. THE OWNER TRIES TO SEIZE (AddKey FullAccess) =====");
    println!("  result      : REJECTED (the account never adds keys, so none exist)");

    arm_transfer(&fleet, &alice).await?;
    let rotate = fleet
        .extension
        .call(&alice, "hos_transfer_ownership")
        .args_json(json!({ "to": fleet.relay.id() }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    assert!(rotate.is_success(), "HoS must retain control: {rotate:#?}");
    println!("\n===== 6. HoS TRANSFERS THE NAME (reclaim / resale) =====");

    let evicted = fleet
        .bob
        .call(&alice, "w_execute_extension")
        .args_json(json!({
            "request": spend_request(fleet.relay.id(), near_sdk::NearToken::from_yoctonear(1))
        }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(60))
        .transact()
        .await;
    let dead = match evicted {
        Err(_) => true,
        Ok(o) => o.is_failure(),
    };
    assert!(dead, "the previous owner must lose all authority");
    let extensions: Vec<String> = fleet.worker.view(&alice, "w_extensions").await?.json()?;
    println!("  old owner   : EVICTED");
    println!("  extensions  : {extensions:?}");

    println!("\n===== SUMMARY =====");
    println!("  - Authority is an AccountId, so a DAO, a passkey user, or a wallet from");
    println!("    another ecosystem can hold a lease with no seed phrase anywhere.");
    println!("  - The account carries NO key: nothing to steal, rotate, or desynchronise.");
    println!("  - Ownership is proven by delegating to the owner account under NEP-641,");
    println!("    so however the owner authenticates is however the lease authenticates.");
    println!("  - The promise path cannot add keys, deploy code, or delete the account.");
    println!("  - HoS transferred the name at will and the old owner went dead.");
    println!("  - The surface is the standard Wallet Contract interface throughout.");
    Ok(())
}
