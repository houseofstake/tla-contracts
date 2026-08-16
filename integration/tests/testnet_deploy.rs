mod common;

use anyhow::{bail, Context, Result};
use common::*;
use near_sdk::json_types::{U128, U64};
use near_workspaces::network::Testnet;
use near_workspaces::types::{Gas, KeyType, NearToken, SecretKey};
use near_workspaces::{Account, AccountId, Worker};
use serde_json::json;
use sha2::{Digest, Sha256};

/// Recovery watchers still hold real ed25519 keys; leased accounts do not.
const WATCHER_KEY: &str = "ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847";
const ROOT: &str = "hosdemo.testnet";
const DEFAULT_RPC: &str = "https://test.rpc.fastnear.com";
const MIN_LABEL_LEN: u8 = 3;
const WALLET_TIMEOUT_SECS: u32 = 3600;
const IMPL_BALANCE: NearToken = NearToken::from_near(32);
const IMPL_FLOOR: NearToken = NearToken::from_near(35);
const REGISTRY_BALANCE: NearToken = NearToken::from_near(10);
const SMALL_BALANCE: NearToken = NearToken::from_millinear(1_500);
const LEASE_DEPOSIT: NearToken = NearToken::from_millinear(7);
const USER_TOP_UP: NearToken = NearToken::from_millinear(100);

fn credentials_path(account: &str) -> Result<String> {
    let home = std::env::var("HOME").context("HOME not set")?;
    Ok(format!("{home}/.near-credentials/testnet/{account}.json"))
}

fn load_key(account: &str) -> Option<SecretKey> {
    let raw = std::fs::read_to_string(credentials_path(account).ok()?).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value["private_key"].as_str()?.parse().ok()
}

fn save_key(account: &AccountId, key: &SecretKey) -> Result<()> {
    let body = json!({
        "account_id": account.as_str(),
        "public_key": key.public_key().to_string(),
        "private_key": key.to_string(),
    });
    std::fs::write(
        credentials_path(account.as_str())?,
        serde_json::to_string_pretty(&body)?,
    )?;
    Ok(())
}

async fn ensure_subaccount(
    worker: &Worker<Testnet>,
    root: &Account,
    label: &str,
    balance: NearToken,
) -> Result<Account> {
    let id: AccountId = format!("{label}.{ROOT}").parse()?;
    if let Some(key) = load_key(id.as_str()) {
        println!("  reusing {id}");
        return Ok(Account::from_secret_key(id, key, worker));
    }
    let account = root
        .create_subaccount(label)
        .keys(SecretKey::from_random(KeyType::ED25519))
        .initial_balance(balance)
        .transact()
        .await?
        .into_result()?;
    save_key(account.id(), account.secret_key())?;
    println!("  created {} with {balance}", account.id());
    Ok(account)
}

async fn is_live(worker: &Worker<Testnet>, id: &AccountId) -> bool {
    worker.view(id, "config").await.is_ok()
}

async fn published_hash(worker: &Worker<Testnet>, id: &AccountId) -> Option<String> {
    worker
        .view(id, "current_hash")
        .await
        .ok()?
        .json::<Option<String>>()
        .ok()?
}

async fn balance_of(worker: &Worker<Testnet>, id: &AccountId) -> Result<NearToken> {
    Ok(worker.view_account(id).await?.balance)
}

async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
}

async fn settled_account(worker: &Worker<Testnet>, id: &AccountId) -> Result<(u128, u64)> {
    for _ in 0..12 {
        if let Ok(view) = worker.view_account(id).await {
            return Ok((view.balance.as_yoctonear(), view.storage_usage));
        }
        settle().await;
    }
    anyhow::bail!("{id} did not become queryable")
}

async fn settled_extensions(worker: &Worker<Testnet>, id: &AccountId) -> Result<Vec<String>> {
    for _ in 0..12 {
        if let Ok(view) = worker.view(id, "w_extensions").await {
            if let Ok(extensions) = view.json::<Vec<String>>() {
                return Ok(extensions);
            }
        }
        settle().await;
    }
    anyhow::bail!("{id} extensions did not become queryable")
}

#[tokio::test]
#[ignore = "deploys to live testnet; run explicitly"]
async fn deploy_demo_fleet_to_testnet() -> Result<()> {
    let rpc = std::env::var("NEAR_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC.to_string());
    println!("rpc {rpc}");
    let worker = near_workspaces::testnet().rpc_addr(&rpc).await?;
    let root_key = load_key(ROOT).context("hosdemo.testnet credentials not found")?;
    let root_id: AccountId = ROOT.parse()?;
    let root = Account::from_secret_key(root_id.clone(), root_key, &worker);
    println!("root {ROOT}: {}", balance_of(&worker, &root_id).await?);

    println!("\n== accounts ==");
    let deployer = ensure_subaccount(&worker, &root, "impl", IMPL_BALANCE).await?;
    let council = ensure_subaccount(&worker, &root, "council", SMALL_BALANCE).await?;
    let registry = ensure_subaccount(&worker, &root, "registry", REGISTRY_BALANCE).await?;
    let extension = ensure_subaccount(&worker, &root, "ext", SMALL_BALANCE).await?;
    let recovery = ensure_subaccount(&worker, &root, "rec", SMALL_BALANCE).await?;
    let deployer_id = deployer.id().clone();

    println!("\n== wallet impl deployer ==");
    if is_live(&worker, &deployer_id).await {
        println!("  already initialized, skipping");
    } else {
        deployer
            .deploy(&wasm("wallet_impl_deployer"))
            .await?
            .into_result()?;
        deployer
            .call(&deployer_id, "new")
            .args_json(json!({ "council": council.id() }))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        println!("  deployed {deployer_id}");
    }

    println!("\n== publish tenant wallet as a global contract ==");
    let wallet_wasm = wasm("hos_wallet");
    let wallet_hash = bs58::encode(Sha256::digest(&wallet_wasm)).into_string();
    let publish_cost =
        NearToken::from_yoctonear(wallet_wasm.len() as u128 * common::GLOBAL_CODE_COST_PER_BYTE);
    if published_hash(&worker, &deployer_id).await.as_deref() == Some(wallet_hash.as_str()) {
        println!("  already published {wallet_hash}");
    } else {
        println!("  {} bytes, cost {publish_cost}", wallet_wasm.len());
        let held = balance_of(&worker, &deployer_id).await?;
        if held < IMPL_FLOOR {
            let top_up = IMPL_FLOOR.saturating_sub(held);
            root.transfer_near(&deployer_id, top_up)
                .await?
                .into_result()?;
            println!("  topped up {deployer_id} by {top_up}");
        }
        council
            .call(&deployer_id, "gd_approve")
            .args_json(json!({ "hash": wallet_hash }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        deployer
            .call(&deployer_id, "gd_deploy")
            .args_json(json!({ "code": wallet_wasm }))
            .deposit(publish_cost)
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        println!("  published {wallet_hash}");
    }

    println!("\n== registrar on the TLA ==");
    // The state struct is unchanged since the fleet was first deployed, so the
    // code can be refreshed in place and the existing state still deserializes.
    let registrar_live = is_live(&worker, &root_id).await;
    root.deploy(&wasm("registrar")).await?.into_result()?;
    if registrar_live {
        println!("  code refreshed on {ROOT}, state kept");
    } else {
        root.call(&root_id, "new")
            .args_json(json!({ "config": {
                "registry": registry.id(),
                "council": council.id(),
                "wallet_impl": deployer_id,
                "hos_extension": extension.id(),
                "recovery": recovery.id(),
                "chain_id": "testnet",
                "min_balance": LEASE_DEPOSIT,
                "min_label_len": MIN_LABEL_LEN,
                "wallet_timeout_secs": WALLET_TIMEOUT_SECS,
            }}))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        println!("  deployed registrar on {ROOT}");
    }

    println!("\n== live mint: the owner is an account, not a key ==");
    let now_ns = worker.view_block().await?.timestamp();
    let label = format!("demo{}", now_ns % 100_000);
    let minted = registry
        .call(&root_id, "create_sub_account")
        .args_json(json!({
            "name": label,
            "owner_account": root_id,
            "payout_account": root_id,
            "lease_until_ns": now_ns + YEAR_NS,
        }))
        .deposit(LEASE_DEPOSIT)
        .max_gas()
        .transact()
        .await?;
    minted.clone().into_result()?;
    let outcome: String = minted.json()?;
    let tenant: AccountId = format!("{label}.{ROOT}").parse()?;
    println!("  mint outcome {outcome} -> {tenant}");
    println!("  owner        {root_id} (no seed phrase exists for this account)");

    let extensions = settled_extensions(&worker, &tenant).await?;
    let keys = worker.view_access_keys(&tenant).await?;
    assert!(
        keys.is_empty(),
        "a leased account must carry no access key at all, found {keys:?}"
    );
    println!("  access keys  none (nothing on-chain to steal or rotate)");
    println!("  extensions   {extensions:?}");

    let (opened_balance, storage_usage) = settled_account(&worker, &tenant).await?;
    let locked = storage_usage as u128 * 10_000_000_000_000_000_000;
    println!("\n== what the account cost to open ==");
    println!("  deposit      {LEASE_DEPOSIT}");
    println!("  storage      {storage_usage} bytes");
    println!("  locked       {}", NearToken::from_yoctonear(locked));
    println!(
        "  free         {}",
        NearToken::from_yoctonear(opened_balance.saturating_sub(locked))
    );

    println!("\n== the user funds their own account ==");
    root.transfer_near(&tenant, USER_TOP_UP)
        .await?
        .into_result()?;
    println!("  {USER_TOP_UP} added by the account owner, not by HoS");

    println!("\n== the owner account drives a real testnet transaction ==");
    let sent = root
        .call(&tenant, "w_execute_extension")
        .args_json(json!({
            "request": spend_request(registry.id(), near_sdk::NearToken::from_millinear(10))
        }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(Gas::from_tgas(100))
        .transact()
        .await?;
    sent.clone().into_result()?;
    let hash = sent.outcome().transaction_hash;
    println!("  https://testnet.nearblocks.io/txns/{hash}");

    let manifest = json!({
        "network": "testnet",
        "tla": ROOT,
        "registrar": ROOT,
        "registry": registry.id(),
        "council": council.id(),
        "wallet_impl": deployer_id,
        "hos_extension": extension.id(),
        "recovery": recovery.id(),
        "wallet_global_hash": wallet_hash,
        "min_label_len": MIN_LABEL_LEN,
        "lease_deposit_yocto": LEASE_DEPOSIT.as_yoctonear().to_string(),
    });
    let path = "../deployment.testnet.json";
    std::fs::write(path, serde_json::to_string_pretty(&manifest)?)?;
    println!("\n== manifest written to {path} ==");
    println!("{}", serde_json::to_string_pretty(&manifest)?);
    Ok(())
}

const NEAR_USD_MICRO: u128 = 5_000_000;
const GRACE_NS: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;
const CONTRACT_FLOOR: NearToken = NearToken::from_near(6);
const CREATION_DEPOSIT_DEFAULT: u128 = 10_000_000_000_000_000_000_000;

fn account_from_creds(worker: &Worker<Testnet>, label: &str) -> Result<Account> {
    let id: AccountId = format!("{label}.{ROOT}").parse()?;
    let key = load_key(id.as_str()).with_context(|| format!("{id} credentials not found"))?;
    Ok(Account::from_secret_key(id, key, worker))
}

async fn ensure_funded(
    worker: &Worker<Testnet>,
    root: &Account,
    id: &AccountId,
    floor: NearToken,
) -> Result<()> {
    let held = balance_of(worker, id).await?;
    if held < floor {
        let top = floor.saturating_sub(held);
        root.transfer_near(id, top).await?.into_result()?;
        println!("  topped up {id} by {top}");
    }
    Ok(())
}

async fn has_code(worker: &Worker<Testnet>, id: &AccountId, view: &str) -> bool {
    worker.view(id, view).await.is_ok()
}

// Stands up the full marketplace layer (tla-registry + hos-extension + mpc-recovery) on top of
// the demo core (registrar + wallet-impl-deployer + global wallet), reusing the hosdemo accounts.
// Idempotent: skips any contract already carrying code. Deposit alignment (finding 1) and the
// recovery dependency of buy/force-transfer (finding 2) are enforced as guards.
#[tokio::test]
#[ignore = "deploys to live testnet; run explicitly"]
async fn deploy_marketplace_to_testnet() -> Result<()> {
    let rpc = std::env::var("NEAR_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC.to_string());
    let worker = near_workspaces::testnet().rpc_addr(&rpc).await?;
    let root_id: AccountId = ROOT.parse()?;
    let root = Account::from_secret_key(
        root_id.clone(),
        load_key(ROOT).context("root creds")?,
        &worker,
    );
    let council = account_from_creds(&worker, "council")?;
    let registry = account_from_creds(&worker, "registry")?;
    let extension = account_from_creds(&worker, "ext")?;
    let recovery = account_from_creds(&worker, "rec")?;
    let registrar_id = root_id.clone();

    println!("== deposit alignment (finding 1) ==");
    let reg_min: NearToken = worker.view(&registrar_id, "min_balance").await?.json()?;
    if reg_min.as_yoctonear() > CREATION_DEPOSIT_DEFAULT {
        anyhow::bail!(
            "registrar min_balance {reg_min} exceeds tla-registry default creation deposit {}; \
             rent will panic at mint. Reconcile update_fee_config account_creation_deposit_yocto \
             >= registrar min_balance before deploying.",
            NearToken::from_yoctonear(CREATION_DEPOSIT_DEFAULT)
        );
    }
    println!(
        "  registrar min_balance {reg_min} <= default creation deposit {} (ok)",
        NearToken::from_yoctonear(CREATION_DEPOSIT_DEFAULT)
    );

    println!("\n== fund marketplace accounts for contract storage ==");
    for acc in [registry.id(), extension.id(), recovery.id()] {
        ensure_funded(&worker, &root, acc, CONTRACT_FLOOR).await?;
    }

    println!("\n== hos-extension ==");
    if has_code(&worker, extension.id(), "get_version").await {
        println!("  already deployed, skipping");
    } else {
        extension
            .deploy(&wasm("hos_extension"))
            .await?
            .into_result()?;
        extension
            .call(extension.id(), "new")
            .args_json(json!({
                "admin": council.id(),
                "registry": registry.id(),
                "recovery": recovery.id(),
                "treasury": council.id(),
            }))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        println!("  deployed {}", extension.id());
    }

    println!("\n== mpc-recovery (finding 2: buy/force-transfer depends on this) ==");
    if has_code(&worker, recovery.id(), "threshold").await {
        println!("  already deployed, skipping");
    } else {
        // Demo config: single placeholder watcher at threshold 1. Replace watchers/threshold with
        // the real quorum before treating recovery as launch-ready (see workbook P4).
        let watcher = WATCHER_KEY.to_string();
        recovery
            .deploy(&wasm("mpc_recovery"))
            .await?
            .into_result()?;
        recovery
            .call(recovery.id(), "new")
            .args_json(json!({
                "owner": council.id(),
                "signer": "v1.signer-prod.testnet",
                "transfer_authority": extension.id(),
                "watchers": [watcher],
                "threshold": 1u32,
            }))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        println!("  deployed {} (placeholder watcher)", recovery.id());
    }

    println!("\n== tla-registry ==");
    if has_code(&worker, registry.id(), "get_fee_config").await {
        println!("  already deployed, skipping");
    } else {
        registry
            .deploy(&wasm("tla_registry"))
            .await?
            .into_result()?;
        registry
            .call(registry.id(), "new")
            .args_json(json!({
                "admin": council.id(),
                "hos_extension": extension.id(),
                "grace_period_ns": U64(GRACE_NS),
                "treasury": council.id(),
                "council": council.id(),
            }))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        println!("  deployed {}", registry.id());
    }

    println!("\n== activate the TLA in tla-registry ==");
    let _ = council
        .call(registry.id(), "admin_set_initial_rate")
        .args_json(json!({ "rate": U128(NEAR_USD_MICRO) }))
        .max_gas()
        .transact()
        .await?;
    if worker
        .view(registry.id(), "get_tla")
        .args_json(json!({ "tla_id": registrar_id }))
        .await?
        .json::<Option<serde_json::Value>>()?
        .is_none()
    {
        council
            .call(registry.id(), "register_tla")
            .args_json(json!({
                "tla_id": registrar_id,
                "tla_type": "Open",
                "premium_category": "Standard",
                "licensee": null,
            }))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        council
            .call(registry.id(), "activate_open_tla")
            .args_json(json!({ "tla_id": registrar_id }))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        println!("  registered + activated {registrar_id}");
    } else {
        println!("  {registrar_id} already registered");
    }

    println!("\n== wire name recovery (quorum lives in mpc-recovery, registry must accept it) ==");
    council
        .call(recovery.id(), "set_registry")
        .args_json(json!({ "registry": registry.id() }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    council
        .call(registry.id(), "add_recovery_authority")
        .args_json(json!({ "account_id": recovery.id() }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    let wired: Option<String> = worker.view(recovery.id(), "registry").await?.json()?;
    let authorities: Vec<String> = worker
        .view(registry.id(), "get_recovery_authorities")
        .await?
        .json()?;
    if wired.as_deref() != Some(registry.id().as_str())
        || !authorities.iter().any(|a| a == recovery.id().as_str())
    {
        bail!("name recovery is not wired: registry={wired:?} authorities={authorities:?}");
    }
    println!("  {} <-> {} both directions", recovery.id(), registry.id());

    println!("\n== venue allowlist (a receiver we never named is a plain new owner) ==");
    match std::env::var("INTENTS_VERIFIER") {
        Ok(venue) if !venue.is_empty() => {
            council
                .call(registry.id(), "add_venue")
                .args_json(json!({ "account_id": venue }))
                .deposit(NearToken::from_yoctonear(1))
                .max_gas()
                .transact()
                .await?
                .into_result()?;
            println!("  named {venue}");
        }
        _ => println!("  INTENTS_VERIFIER unset, skipping; deposits will settle as plain transfers"),
    }

    println!("\n== deployment readiness ==");
    let readiness: serde_json::Value = worker
        .view(registry.id(), "deployment_readiness")
        .await?
        .json()?;
    println!("  {readiness}");
    if readiness["ready"] != serde_json::Value::Bool(true) {
        println!("  NOT READY: every false field above is an unfinished deploy step");
    }

    println!(
        "\n== marketplace fleet live: registry={} ext={} rec={} ==",
        registry.id(),
        extension.id(),
        recovery.id()
    );
    Ok(())
}
