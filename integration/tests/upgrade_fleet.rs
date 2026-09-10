mod common;

use anyhow::{bail, Context, Result};
use common::{account, wasm};
use near_workspaces::types::{Gas, SecretKey};
use near_workspaces::{Account, AccountId, ContractState};
use serde_json::json;
use sha2::{Digest, Sha256};

const DEFAULT_RPC: &str = "https://test.rpc.fastnear.com";

type Testnet = near_workspaces::Worker<near_workspaces::network::Testnet>;

const FLEET_ARTIFACTS: [&str; 5] = [
    "wallet_impl_deployer",
    "registrar",
    "hos_extension",
    "mpc_recovery",
    "tla_registry",
];

const FLEET_VARS: [&str; 5] = [
    "DEPLOYER_ACCOUNT",
    "ROOT_ACCOUNT",
    "EXTENSION_ACCOUNT",
    "RECOVERY_ACCOUNT",
    "REGISTRY_ACCOUNT",
];

fn fleet() -> [(String, &'static str); 5] {
    std::array::from_fn(|i| (account(FLEET_VARS[i]), FLEET_ARTIFACTS[i]))
}

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

async fn registry_state(worker: &Testnet) -> Result<serde_json::Value> {
    let registry: AccountId = account("REGISTRY_ACCOUNT").parse()?;
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

    for (target, artifact) in fleet() {
        let id: AccountId = target.parse()?;
        let code = wasm(artifact);
        let want = bs58::encode(Sha256::digest(&code)).into_string();
        let have = code_hash(&worker, &id).await?;

        if have == want {
            println!("{target:<30} already on {want}, skipping");
            skipped += 1;
            continue;
        }

        let signer = Account::from_secret_key(id.clone(), load_key(&target)?, &worker);
        println!("{target:<30} {have} -> {want}");

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
            bail!("{target}: deploy and migrate failed: {outcome:#?}");
        }
        if let Some(failure) = outcome.receipt_failures().first() {
            bail!("{target}: migrate receipt failed: {failure:?}");
        }

        let now = code_hash(&worker, &id).await?;
        if now != want {
            bail!("{target}: deployed but on-chain hash is {now}, expected {want}");
        }
        println!("{target:<30} migrated and verified");
        changed += 1;
    }

    let after = registry_state(&worker).await?;
    println!("registry after:  {after}");
    if let Some(before) = before {
        for field in [
            "tla_count",
            "sub_account_count",
            "total_pending_refunds_yocto",
        ] {
            if before[field] != after[field] {
                bail!(
                    "{field} changed across the upgrade: {} -> {}",
                    before[field],
                    after[field]
                );
            }
        }
    }

    let registry: AccountId = account("REGISTRY_ACCOUNT").parse()?;
    let readiness: serde_json::Value = worker
        .view(&registry, "deployment_readiness")
        .await?
        .json()?;
    if readiness["ready"] != json!(true) {
        bail!("fleet is not ready after the upgrade: {readiness}");
    }

    println!("\n{changed} upgraded, {skipped} already current, readiness {readiness}");
    println!("run publish_fleet next to publish the wallet and migrate leased accounts");
    Ok(())
}

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
            FLEET_ARTIFACTS.contains(&artifact),
            "{artifact} is deployed but missing from the fleet list, so an upgrade would skip it"
        );
    }
    assert_eq!(
        FLEET_ARTIFACTS[0], "wallet_impl_deployer",
        "the deployer must be first: nothing can be published until it takes the current gd_deploy shape"
    );
    assert_eq!(
        FLEET_ARTIFACTS[FLEET_ARTIFACTS.len() - 1],
        "tla_registry",
        "the registry must be last: it drives the others"
    );
    assert_eq!(
        FLEET_VARS.len(),
        FLEET_ARTIFACTS.len(),
        "every fleet artifact needs the variable naming the account it deploys to"
    );
}
