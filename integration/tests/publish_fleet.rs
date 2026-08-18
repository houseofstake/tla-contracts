mod common;

use anyhow::{bail, Context, Result};
use common::{wasm, GLOBAL_CODE_COST_PER_BYTE};
use near_workspaces::types::{NearToken, SecretKey};
use near_workspaces::{Account, AccountId};
use serde_json::json;
use sha2::{Digest, Sha256};

const DEFAULT_RPC: &str = "https://test.rpc.fastnear.com";
const REGISTRY: &str = "registry.hosdemo.testnet";
const EXTENSION: &str = "ext.hosdemo.testnet";
const DEPLOYER: &str = "impl.hosdemo.testnet";
const COUNCIL: &str = "council.hosdemo.testnet";

type Testnet = near_workspaces::Worker<near_workspaces::network::Testnet>;

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

async fn leased_names(worker: &Testnet) -> Result<Vec<String>> {
    let registry: AccountId = REGISTRY.parse()?;
    let mut names = Vec::new();
    let mut from = 0u64;
    loop {
        let page: Vec<serde_json::Value> = worker
            .view(&registry, "nft_tokens")
            .args_json(json!({ "from_index": from.to_string(), "limit": 50 }))
            .await?
            .json()?;
        let count = page.len();
        for row in &page {
            names.push(
                row.get("token_id")
                    .and_then(|v| v.as_str())
                    .context("nft_tokens row without a token_id")?
                    .to_string(),
            );
        }
        if count < 50 {
            break;
        }
        from += 50;
    }
    Ok(names)
}

async fn impl_version(worker: &Testnet, name: &str) -> Option<u64> {
    let id: AccountId = name.parse().ok()?;
    let view = worker.view(&id, "hos_lease").await.ok()?;
    let lease: serde_json::Value = view.json().ok()?;
    lease.get("impl_version")?.as_u64()
}

#[tokio::test]
async fn publish_and_migrate_the_fleet() -> Result<()> {
    let target: u64 = std::env::var("TARGET_IMPL_VERSION")
        .context("set TARGET_IMPL_VERSION to the version this wasm reports")?
        .parse()?;

    let worker = near_workspaces::testnet().rpc_addr(&rpc_url()).await?;
    let council = Account::from_secret_key(COUNCIL.parse()?, load_key(COUNCIL)?, &worker);
    let deployer_id: AccountId = DEPLOYER.parse()?;
    let extension_id: AccountId = EXTENSION.parse()?;

    let code = wasm("hos_wallet");
    let hash = bs58::encode(Sha256::digest(&code)).into_string();
    let cost = NearToken::from_yoctonear(code.len() as u128 * GLOBAL_CODE_COST_PER_BYTE);
    println!("wallet {} bytes, hash {hash}, cost {cost}", code.len());

    let names = leased_names(&worker).await?;
    println!("fleet: {} names", names.len());
    if names.is_empty() {
        bail!("refusing to publish against an empty collection");
    }

    let published: Option<String> = worker
        .view(&deployer_id, "current_hash")
        .await?
        .json()
        .unwrap_or(None);
    if published.as_deref() == Some(hash.as_str()) {
        println!("already published, skipping approve and deploy");
    } else {
        council
            .call(&deployer_id, "gd_approve")
            .args_json(json!({ "hash": hash }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        println!("approved {hash}");

        council
            .call(&deployer_id, "gd_deploy")
            .args_json(json!({ "code": common::code_arg(&code) }))
            .deposit(cost)
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        println!("published {hash}");
    }

    println!("\nmigrating {} accounts", names.len());
    let mut migrated = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for name in &names {
        if impl_version(&worker, name).await == Some(target) {
            migrated += 1;
            continue;
        }
        let outcome = council
            .call(&extension_id, "migrate_wallet")
            .args_json(json!({ "wallet": name }))
            .max_gas()
            .transact()
            .await;
        match outcome {
            Ok(result) if result.is_success() => {
                migrated += 1;
                if migrated.is_multiple_of(10) {
                    println!("  {migrated}/{}", names.len());
                }
            }
            Ok(result) => {
                println!("  refused {name}: {:?}", result.into_result().err());
                failed.push(name.clone());
            }
            Err(err) => {
                println!("  error {name}: {err}");
                failed.push(name.clone());
            }
        }
    }

    println!("\nretrying {} that did not land", failed.len());
    let mut still_failed = Vec::new();
    for name in &failed {
        if impl_version(&worker, name).await == Some(target) {
            continue;
        }
        let outcome = council
            .call(&extension_id, "migrate_wallet")
            .args_json(json!({ "wallet": name }))
            .max_gas()
            .transact()
            .await;
        match outcome {
            Ok(result) if result.is_success() => println!("  recovered {name}"),
            _ => still_failed.push(name.clone()),
        }
    }

    let mut unmigrated = Vec::new();
    for name in &names {
        if impl_version(&worker, name).await != Some(target) {
            unmigrated.push(name.clone());
        }
    }

    println!(
        "\non impl_version {target}: {}/{}",
        names.len() - unmigrated.len(),
        names.len()
    );
    if !unmigrated.is_empty() {
        for name in unmigrated.iter().take(20) {
            println!("  NOT MIGRATED {name}");
        }
        bail!(
            "{} accounts are running the new code against old state and cannot be \
             read until they migrate. Re-run this test; it skips those already done.",
            unmigrated.len()
        );
    }
    if !still_failed.is_empty() {
        bail!(
            "{} accounts needed a retry that did not land",
            still_failed.len()
        );
    }

    println!("\nfleet published and migrated");
    Ok(())
}
