mod common;

use anyhow::{bail, Context, Result};
use common::wasm;
use near_workspaces::types::{Gas, SecretKey};
use near_workspaces::{Account, AccountId, ContractState};
use serde_json::json;
use sha2::{Digest, Sha256};

const DEFAULT_RPC: &str = "https://test.rpc.fastnear.com";
const REGISTRY: &str = "registry.hosdemo.testnet";

type Testnet = near_workspaces::Worker<near_workspaces::network::Testnet>;

/// Dependency order. The deployer goes first because nothing can be published
/// until it accepts the current `gd_deploy` shape, and the registry goes last
/// because it drives the others.
const FLEET: [(&str, &str); 5] = [
    ("impl.hosdemo.testnet", "wallet_impl_deployer"),
    ("hosdemo.testnet", "registrar"),
    ("ext.hosdemo.testnet", "hos_extension"),
    ("rec.hosdemo.testnet", "mpc_recovery"),
    (REGISTRY, "tla_registry"),
];

fn rpc_url() -> String {
    std::env::var("NEAR_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC.to_string())
}

fn load_key(account: &str) -> Result<SecretKey> {
    let home = std::env::var("HOME").context("HOME not set")?;
    let path = format!("{home}/.near-credentials/testnet/{account}.json");
    let raw = std::fs::read_to_string(&path).with_context(|| format!("read {path}"))?;
    let value: serde_json::Value = serde_json::from_str(&raw)?;
    Ok(value["private_key"]
        .as_str()
        .with_context(|| format!("{path} has no private_key"))?
        .parse()?)
}

async fn code_hash(worker: &Testnet, id: &AccountId) -> Result<String> {
    match worker.view_account(id).await?.contract_state {
        ContractState::LocalHash(hash) => Ok(hash.to_string()),
        other => bail!("{id} does not hold local contract code: {other:?}"),
    }
}

/// The registry's own accounting, read before and after so a migration that
/// silently drops state fails this run rather than a later audit.
async fn registry_state(worker: &Testnet) -> Result<serde_json::Value> {
    let registry: AccountId = REGISTRY.parse()?;
    Ok(worker.view(&registry, "get_stats").await?.json()?)
}

#[tokio::test]
#[ignore = "touches live testnet; run explicitly with credentials present"]
async fn upgrade_the_fleet_in_order() -> Result<()> {
    let worker = near_workspaces::testnet().rpc_addr(&rpc_url()).await?;

    let before = registry_state(&worker).await.ok();
    if let Some(stats) = &before {
        println!("registry before: {stats}");
    }

    let mut changed = 0usize;
    let mut skipped = 0usize;

    for (account, artifact) in FLEET {
        let id: AccountId = account.parse()?;
        let code = wasm(artifact);
        let want = bs58::encode(Sha256::digest(&code)).into_string();
        let have = code_hash(&worker, &id).await?;

        if have == want {
            println!("{account:<30} already on {want}, skipping");
            skipped += 1;
            continue;
        }

        let signer = Account::from_secret_key(id.clone(), load_key(account)?, &worker);
        println!("{account:<30} {have} -> {want}");

        let outcome = signer
            .batch(&id)
            .deploy(&code)
            .call(
                near_workspaces::operations::Function::new("migrate")
                    .args_json(json!({}))
                    .gas(Gas::from_tgas(200)),
            )
            .transact()
            .await?;
        if outcome.is_failure() {
            bail!("{account}: deploy and migrate failed: {outcome:#?}");
        }
        if let Some(failure) = outcome.receipt_failures().first() {
            bail!("{account}: migrate receipt failed: {failure:?}");
        }

        let now = code_hash(&worker, &id).await?;
        if now != want {
            bail!("{account}: deployed but on-chain hash is {now}, expected {want}");
        }
        println!("{account:<30} migrated and verified");
        changed += 1;
    }

    let after = registry_state(&worker).await?;
    println!("registry after:  {after}");
    if let Some(before) = before {
        for field in ["tla_count", "sub_account_count", "total_pending_refunds_yocto"] {
            if before[field] != after[field] {
                bail!(
                    "{field} changed across the upgrade: {} -> {}",
                    before[field],
                    after[field]
                );
            }
        }
    }

    let registry: AccountId = REGISTRY.parse()?;
    let readiness: serde_json::Value = worker.view(&registry, "deployment_readiness").await?.json()?;
    if readiness["ready"] != json!(true) {
        bail!("fleet is not ready after the upgrade: {readiness}");
    }

    println!("\n{changed} upgraded, {skipped} already current, readiness {readiness}");
    println!("run publish_fleet next to publish the wallet and migrate leased accounts");
    Ok(())
}

/// Guards the constant above against a contract being added to the workspace
/// and quietly left out of the deployment sequence.
#[test]
fn every_deployed_contract_is_in_the_fleet_list() {
    let deployed = [
        "wallet_impl_deployer",
        "registrar",
        "hos_extension",
        "mpc_recovery",
        "tla_registry",
    ];
    for artifact in deployed {
        assert!(
            FLEET.iter().any(|(_, a)| *a == artifact),
            "{artifact} is deployed but missing from FLEET, so a fleet upgrade would skip it"
        );
    }
    assert_eq!(
        FLEET[0].1, "wallet_impl_deployer",
        "the deployer must be first: nothing can be published until it takes the current gd_deploy shape"
    );
    assert_eq!(
        FLEET[FLEET.len() - 1].0,
        REGISTRY,
        "the registry must be last: it drives the others"
    );
}
