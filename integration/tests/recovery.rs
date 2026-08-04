mod common;

use anyhow::Result;
use common::*;
use defuse_wallet_ed25519::crypto::ed25519::ed25519_dalek::Signer as DalekSigner;
use defuse_wallet_ed25519::crypto::ed25519::ed25519_dalek::SigningKey;
use near_sdk::json_types::{Base64VecU8, U64};
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
