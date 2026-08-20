mod common;

use anyhow::{bail, Result};
use common::*;
use defuse_wallet_ed25519::crypto::ed25519::ed25519_dalek::Signer as DalekSigner;
use defuse_wallet_ed25519::crypto::ed25519::ed25519_dalek::SigningKey;
use defuse_wallet::actions::FunctionCall;
use defuse_wallet::{NearPromise, Request};
use near_sdk::json_types::{Base64VecU8, U64};
use near_workspaces::types::NearToken;
use near_workspaces::Contract;
use serde_json::json;

const DOMAIN_REQUEST: u8 = 1;
const DOMAIN_VERDICT: u8 = 2;
const TIMELOCK_SECS: u32 = 60;

fn watcher_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn pubkey_str(k: &SigningKey) -> String {
    format!(
        "ed25519:{}",
        bs58::encode(k.verifying_key().to_bytes()).into_string()
    )
}

fn pubkey_33(k: &SigningKey) -> Vec<u8> {
    let mut out = vec![0u8];
    out.extend_from_slice(&k.verifying_key().to_bytes());
    out
}

fn push_str(m: &mut Vec<u8>, s: &str) {
    m.extend_from_slice(&(s.len() as u32).to_le_bytes());
    m.extend_from_slice(s.as_bytes());
}

fn message_core(
    domain: u8,
    contract: &str,
    account: &str,
    new_owner: &[u8],
    round: u64,
) -> Vec<u8> {
    let mut m = vec![domain];
    push_str(&mut m, contract);
    push_str(&mut m, account);
    m.extend_from_slice(new_owner);
    m.extend_from_slice(&round.to_le_bytes());
    m
}

fn sign(k: &SigningKey, message: &[u8]) -> Base64VecU8 {
    Base64VecU8(DalekSigner::sign(k, message).to_bytes().to_vec())
}

async fn deploy_recovery(
    fleet: &Fleet,
    watchers: &[&SigningKey],
    threshold: u32,
) -> Result<Contract> {
    let recovery = fleet
        .recovery
        .deploy(&wasm("mpc_recovery"))
        .await?
        .into_result()?;
    let watcher_keys: Vec<String> = watchers.iter().map(|w| pubkey_str(w)).collect();
    recovery
        .call("new")
        .args_json(json!({
            "owner": fleet.council.id(),
            "signer": fleet.bob.id(),
            "transfer_authority": fleet.extension.id(),
            "watchers": watcher_keys,
            "threshold": threshold,
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    Ok(recovery)
}

fn name_recovery_message(
    contract: &str,
    tla_id: &str,
    name: &str,
    new_owner: &str,
    expected_owner: &str,
    deadline_ns: u64,
    round: u64,
) -> Vec<u8> {
    let mut m = vec![3u8];
    push_str(&mut m, contract);
    push_str(&mut m, tla_id);
    push_str(&mut m, name);
    push_str(&mut m, new_owner);
    push_str(&mut m, expected_owner);
    m.extend_from_slice(&deadline_ns.to_le_bytes());
    m.extend_from_slice(&round.to_le_bytes());
    m
}

fn name_request_message(
    contract: &str,
    tla_id: &str,
    name: &str,
    new_owner: &str,
    round: u64,
) -> Vec<u8> {
    let mut m = vec![4u8];
    push_str(&mut m, contract);
    push_str(&mut m, tla_id);
    push_str(&mut m, name);
    push_str(&mut m, new_owner);
    m.extend_from_slice(&round.to_le_bytes());
    m
}

async fn request_name_recovery(
    fleet: &Fleet,
    recovery: &near_workspaces::Account,
    tla: &near_workspaces::AccountId,
    new_owner: &near_workspaces::AccountId,
    attestation: &SigningKey,
    round: u64,
) -> Result<()> {
    let message = name_request_message(
        recovery.id().as_str(),
        tla.as_str(),
        "alice",
        new_owner.as_str(),
        round,
    );
    let requested = fleet
        .relay
        .call(recovery.id(), "request_name_recovery")
        .args_json(json!({
            "tla_id": tla,
            "name": "alice",
            "new_owner": new_owner,
            "attestation": sign(attestation, &message),
        }))
        .max_gas()
        .transact()
        .await?;
    if let Some(failure) = requested.receipt_failures().first() {
        bail!("an attested name recovery request must land: {failure:?}");
    }
    Ok(())
}

async fn arm_and_install_name_policy(
    fleet: &Fleet,
    recovery: &near_workspaces::Account,
    tenant: &near_workspaces::AccountId,
    attestation: &SigningKey,
) -> Result<()> {
    let target: near_sdk::AccountId = recovery.id().as_str().parse().unwrap();
    let arm = NearPromise::new(target).function_call(
        FunctionCall::name("arm_policy_install")
            .args(
                serde_json::to_vec(&json!({
                    "attestation_key": pubkey_str(attestation),
                    "timelock_secs": TIMELOCK_SECS,
                }))
                .unwrap(),
            )
            .gas(near_sdk::Gas::from_tgas(20))
            .attach_deposit(near_sdk::NearToken::from_yoctonear(0)),
    );
    let armed = fleet
        .bob
        .call(tenant, "w_execute_extension")
        .args_json(json!({ "request": Request::new().external([arm]) }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    if let Some(failure) = armed.receipt_failures().first() {
        bail!("the holder must be able to arm a policy install: {failure:?}");
    }
    fleet
        .council
        .call(recovery.id(), "install_policy")
        .args_json(json!({
            "account": tenant,
            "mpc_public_key": pubkey_str(attestation),
            "attestation_key": pubkey_str(attestation),
            "timelock_secs": TIMELOCK_SECS,
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    Ok(())
}

#[tokio::test]
async fn a_watcher_quorum_recovers_a_leased_name_end_to_end() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let watchers = [watcher_key(41), watcher_key(42), watcher_key(43)];
    let registry = deploy_registry(&fleet).await?;
    let recovery = fleet.recovery.clone();
    let tla = fleet.registrar.id().clone();
    let tenant = rent(&fleet, &registry, &tla, "alice").await?;

    fleet
        .council
        .call(recovery.id(), "set_watchers")
        .args_json(json!({
            "watchers": watchers.iter().map(pubkey_str).collect::<Vec<_>>(),
            "threshold": 2,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    fleet
        .council
        .call(recovery.id(), "set_registry")
        .args_json(json!({ "registry": registry.id() }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    fleet
        .council
        .call(registry.id(), "add_recovery_authority")
        .args_json(json!({ "account_id": recovery.id() }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let attestation = watcher_key(51);
    arm_and_install_name_policy(&fleet, &recovery, &tenant, &attestation).await?;
    request_name_recovery(&fleet, &recovery, &tla, fleet.relay.id(), &attestation, 0).await?;
    fleet.worker.fast_forward(1000).await?;

    let deadline_ns = now_secs() as u64 * 1_000_000_000 + 3_600_000_000_000;
    let message = name_recovery_message(
        recovery.id().as_str(),
        tla.as_str(),
        "alice",
        fleet.relay.id().as_str(),
        fleet.bob.id().as_str(),
        deadline_ns,
        0,
    );
    let signatures: Vec<serde_json::Value> = watchers[..2]
        .iter()
        .map(|w| {
            json!({
                "public_key": pubkey_str(w),
                "signature": sign(w, &message),
            })
        })
        .collect();

    let recovered = fleet
        .relay
        .call(recovery.id(), "recover_name")
        .args_json(json!({
            "tla_id": tla,
            "name": "alice",
            "new_owner": fleet.relay.id(),
            "expected_owner": fleet.bob.id(),
            "deadline_ns": deadline_ns.to_string(),
            "signatures": signatures,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    if let Some(failure) = recovered.receipt_failures().first() {
        bail!("a quorum-backed name recovery must land: {failure:?}");
    }

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.relay.id().as_str(),
        "the leased account itself must follow the recovery"
    );
    let holder: Option<serde_json::Value> = fleet
        .worker
        .view(registry.id(), "nft_token")
        .args_json(json!({ "token_id": tenant.as_str() }))
        .await?
        .json()?;
    assert_eq!(
        holder.as_ref().and_then(|t| t["owner_id"].as_str()),
        Some(fleet.relay.id().as_str()),
        "and the nft record must agree with it"
    );
    Ok(())
}

#[tokio::test]
async fn a_single_watcher_cannot_recover_a_leased_name() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let watchers = [watcher_key(51), watcher_key(52)];
    let registry = deploy_registry(&fleet).await?;
    let recovery = fleet.recovery.clone();
    let tla = fleet.registrar.id().clone();
    let tenant = rent(&fleet, &registry, &tla, "alice").await?;

    fleet
        .council
        .call(recovery.id(), "set_watchers")
        .args_json(json!({
            "watchers": watchers.iter().map(pubkey_str).collect::<Vec<_>>(),
            "threshold": 2,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    fleet
        .council
        .call(recovery.id(), "set_registry")
        .args_json(json!({ "registry": registry.id() }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    fleet
        .council
        .call(registry.id(), "add_recovery_authority")
        .args_json(json!({ "account_id": recovery.id() }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let attestation = watcher_key(61);
    arm_and_install_name_policy(&fleet, &recovery, &tenant, &attestation).await?;
    request_name_recovery(&fleet, &recovery, &tla, fleet.relay.id(), &attestation, 0).await?;

    let deadline_ns = now_secs() as u64 * 1_000_000_000 + 3_600_000_000_000;
    let message = name_recovery_message(
        recovery.id().as_str(),
        tla.as_str(),
        "alice",
        fleet.relay.id().as_str(),
        fleet.bob.id().as_str(),
        deadline_ns,
        0,
    );
    let attempt = fleet
        .relay
        .call(recovery.id(), "recover_name")
        .args_json(json!({
            "tla_id": tla,
            "name": "alice",
            "new_owner": fleet.relay.id(),
            "expected_owner": fleet.bob.id(),
            "deadline_ns": deadline_ns.to_string(),
            "signatures": [{
                "public_key": pubkey_str(&watchers[0]),
                "signature": sign(&watchers[0], &message),
            }],
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    assert!(
        !attempt.is_success() || !attempt.receipt_failures().is_empty(),
        "one watcher must not be able to take a live name"
    );
    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.bob.id().as_str(),
        "the holder keeps the name"
    );
    Ok(())
}

#[tokio::test]
async fn recovery_reaches_approved_for_a_native_account() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let watchers = [watcher_key(21), watcher_key(22), watcher_key(23)];
    let attestation = watcher_key(31);
    let new_owner = watcher_key(41);
    let mpc_key = watcher_key(51);

    let recovery = deploy_recovery(&fleet, &watchers.iter().collect::<Vec<_>>(), 2).await?;

    // Leased accounts no longer carry a recovery policy: their owner is an
    // AccountId, so recovery happens at the owner's own account. What remains
    // is native recovery for ordinary accounts.
    let victim = fleet.bob.id().clone();

    fleet
        .bob
        .call(recovery.id(), "arm_policy_install")
        .args_json(json!({
            "attestation_key": pubkey_str(&attestation),
            "timelock_secs": TIMELOCK_SECS,
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    fleet
        .council
        .call(recovery.id(), "install_policy")
        .args_json(json!({
            "account": victim,
            "mpc_public_key": pubkey_str(&mpc_key),
            "attestation_key": pubkey_str(&attestation),
            "timelock_secs": TIMELOCK_SECS,
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let request_msg = message_core(
        DOMAIN_REQUEST,
        recovery.id().as_str(),
        victim.as_str(),
        &pubkey_33(&new_owner),
        0,
    );
    let requested = fleet
        .bob
        .call(recovery.id(), "request_recovery")
        .args_json(json!({
            "account": victim,
            "new_owner": pubkey_str(&new_owner),
            "round": U64(0),
            "attestation": sign(&attestation, &request_msg),
        }))
        .max_gas()
        .transact()
        .await?;
    assert!(requested.is_success(), "request_recovery: {requested:#?}");

    fleet.worker.fast_forward(1000).await?;

    let verdict_msg = {
        let mut m = message_core(
            DOMAIN_VERDICT,
            recovery.id().as_str(),
            victim.as_str(),
            &pubkey_33(&new_owner),
            0,
        );
        m.push(1);
        m
    };
    let signatures: Vec<_> = watchers[..2]
        .iter()
        .map(|w| json!({ "public_key": pubkey_str(w), "signature": sign(w, &verdict_msg) }))
        .collect();
    let verdict = fleet
        .bob
        .call(recovery.id(), "submit_verdict")
        .args_json(json!({ "account": victim, "verdict": "Approve", "signatures": signatures }))
        .max_gas()
        .transact()
        .await?;
    assert!(verdict.is_success(), "submit_verdict: {verdict:#?}");

    let round: Option<u64> = recovery
        .view("round_of")
        .args_json(json!({ "account": victim }))
        .await?
        .json()?;
    assert_eq!(round, Some(1), "a settled verdict must advance the round");
    Ok(())
}
