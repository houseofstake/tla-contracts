#![allow(dead_code)]

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use defuse_wallet::{NearPromise, Request};
use near_sdk::json_types::{Base64VecU8, U128, U64};
use near_workspaces::network::Sandbox;
use near_workspaces::types::{NearToken, SecretKey};
use near_workspaces::{Account, AccountId, Contract, Worker};
use serde_json::json;
use sha2::{Digest, Sha256};

pub const YEAR_NS: u64 = 31_536_000_000_000_000;
pub const GLOBAL_CODE_COST_PER_BYTE: u128 = 100_000_000_000_000_000_000;

pub fn wasm(name: &str) -> Vec<u8> {
    let path = format!("../target/near/{name}/{name}.wasm");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// `gd_deploy` takes the code base64 encoded. A JSON number array costs about
/// three times as many argument bytes and has to be parsed a number at a time,
/// which put the publish over the per-transaction gas limit.
pub fn code_arg(code: &[u8]) -> Base64VecU8 {
    Base64VecU8::from(code.to_vec())
}

pub fn now_secs() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as u32
}

pub fn lease_until_ns() -> u64 {
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    now_ns + YEAR_NS
}

/// The only way into a leased account: an enabled extension calls as
/// predecessor. There is no key, so nothing signs on the account's behalf.
pub fn transfer_to(to: &AccountId, amount: near_sdk::NearToken) -> NearPromise {
    let id: near_sdk::AccountId = to.as_str().parse().unwrap();
    NearPromise::new(id).transfer(amount)
}

pub fn spend_request(to: &AccountId, amount: near_sdk::NearToken) -> Request {
    Request::new().external([transfer_to(to, amount)])
}

async fn subaccount(root: &Account, name: &str, balance: NearToken) -> Result<Account> {
    Ok(root
        .create_subaccount(name)
        .initial_balance(balance)
        .transact()
        .await?
        .into_result()?)
}

pub async fn balance_of(worker: &Worker<Sandbox>, id: &AccountId) -> Result<u128> {
    Ok(worker.view_account(id).await?.balance.as_yoctonear())
}

pub struct Fleet {
    pub worker: Worker<Sandbox>,
    pub council: Account,
    pub registry: Account,
    pub extension: Account,
    pub recovery: Account,
    pub deployer: Contract,
    pub registrar: Contract,
    pub impl_account: AccountId,
    pub bob: Account,
    pub relay: Account,
}

pub async fn deploy_fleet() -> Result<Fleet> {
    let worker = near_workspaces::sandbox().await?;
    let root = worker.root_account()?;

    let council = subaccount(&root, "council", NearToken::from_near(20)).await?;
    let patch = subaccount(&root, "patch", NearToken::from_near(5)).await?;
    let registry = subaccount(&root, "registry", NearToken::from_near(20)).await?;
    let extension = subaccount(&root, "ext", NearToken::from_near(10)).await?;
    let recovery = subaccount(&root, "rec", NearToken::from_near(10)).await?;
    let impl_owner = subaccount(&root, "w", NearToken::from_near(10)).await?;
    let tla = subaccount(&root, "tla", NearToken::from_near(10)).await?;
    let bob = subaccount(&root, "bob", NearToken::from_near(5)).await?;
    let relay = subaccount(&root, "relay", NearToken::from_near(120)).await?;
    let impl_account = impl_owner.id().clone();

    let deployer = impl_owner
        .deploy(&wasm("wallet_impl_deployer"))
        .await?
        .into_result()?;
    deployer
        .call("new")
        .args_json(json!({
            "council": council.id(),
            "patch_authority": patch.id(),
            "approval_delay_ns": "0",
        }))
        .transact()
        .await?
        .into_result()?;

    let wallet_wasm = wasm("hos_wallet");
    let wallet_hash = bs58::encode(Sha256::digest(&wallet_wasm)).into_string();
    council
        .call(deployer.id(), "gd_approve")
        .args_json(json!({ "hash": wallet_hash }))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await?
        .into_result()?;
    let publish_cost =
        NearToken::from_yoctonear(wallet_wasm.len() as u128 * GLOBAL_CODE_COST_PER_BYTE);
    relay
        .call(deployer.id(), "gd_deploy")
        .args_json(json!({ "code": code_arg(&wallet_wasm) }))
        .deposit(publish_cost)
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let registrar = tla.deploy(&wasm("registrar")).await?.into_result()?;
    registrar
        .call("new")
        .args_json(json!({ "config": {
            "registry": registry.id(),
            "council": council.id(),
            "wallet_impl": impl_account,
            "hos_extension": extension.id(),
            "recovery": recovery.id(),
            "chain_id": "testnet",
            "min_balance": NearToken::from_millinear(100),
            "min_label_len": 3,
            "wallet_timeout_secs": 3600,
        }}))
        .transact()
        .await?
        .into_result()?;

    Ok(Fleet {
        worker,
        council,
        registry,
        extension,
        recovery,
        deployer,
        registrar,
        impl_account,
        bob,
        relay,
    })
}

/// Mints a leased account owned by `fleet.bob`.
pub async fn mint(fleet: &Fleet, name: &str, balance: NearToken) -> Result<AccountId> {
    let owner = fleet.bob.id().clone();
    mint_owned_by(fleet, name, &owner, balance, lease_until_ns()).await
}

pub async fn mint_owned_by(
    fleet: &Fleet,
    name: &str,
    owner: &AccountId,
    balance: NearToken,
    lease_until_ns: u64,
) -> Result<AccountId> {
    let outcome = fleet
        .registry
        .call(fleet.registrar.id(), "create_sub_account")
        .args_json(json!({
            "name": name,
            "owner_account": owner,
            "payout_account": owner,
            "lease_until_ns": lease_until_ns,
        }))
        .deposit(NearToken::from_millinear(500))
        .max_gas()
        .transact()
        .await?;
    assert!(outcome.is_success(), "mint failed: {outcome:#?}");
    let minted: String = outcome.json()?;
    assert_eq!(minted, "Active", "mint outcome was not Active");
    let id: AccountId = format!("{name}.{}", fleet.registrar.id()).parse().unwrap();
    fleet
        .relay
        .transfer_near(&id, balance)
        .await?
        .into_result()?;
    Ok(id)
}

pub async fn chain_timestamp_ns(worker: &Worker<Sandbox>) -> Result<u64> {
    Ok(worker.view_block().await?.timestamp())
}

pub fn account_as(id: &AccountId, secret: &SecretKey, worker: &Worker<Sandbox>) -> Account {
    Account::from_secret_key(id.clone(), secret.clone(), worker)
}
pub const WATCHER_KEY: &str = "ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847";

pub fn second_watcher_key() -> String {
    SecretKey::from_random(near_workspaces::types::KeyType::ED25519)
        .public_key()
        .to_string()
}
pub const NEAR_USD_MICRO: u128 = 5_000_000;
pub const GRACE_NS: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;

pub async fn deploy_registry(fleet: &Fleet) -> Result<Contract> {
    let extension = fleet
        .extension
        .deploy(&wasm("hos_extension"))
        .await?
        .into_result()?;
    extension
        .call("new")
        .args_json(json!({
            "admin": fleet.council.id(),
            "registry": fleet.registry.id(),
            "recovery": fleet.recovery.id(),
            "treasury": fleet.council.id(),
            "council": fleet.council.id(),
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let recovery = fleet
        .recovery
        .deploy(&wasm("mpc_recovery"))
        .await?
        .into_result()?;
    recovery
        .call("new")
        .args_json(json!({
            "owner": fleet.council.id(),
            "signer": fleet.bob.id(),
            "transfer_authority": fleet.extension.id(),
            "watchers": [WATCHER_KEY, second_watcher_key()],
            "threshold": 2,
        }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let registry = fleet
        .registry
        .deploy(&wasm("tla_registry"))
        .await?
        .into_result()?;
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

    let tla = fleet.registrar.id();
    fleet
        .council
        .call(registry.id(), "admin_set_initial_rate")
        .args_json(json!({ "rate": U128(NEAR_USD_MICRO) }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    fleet
        .council
        .call(registry.id(), "register_tla")
        .args_json(json!({
            "tla_id": tla,
            "tla_type": "Open",
            "premium_category": "Standard",
            "licensee": null,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    fleet
        .council
        .call(registry.id(), "activate_open_tla")
        .args_json(json!({ "tla_id": tla }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let mut fee: serde_json::Value = registry.view("get_fee_config").await?.json()?;
    fee["account_creation_deposit_yocto"] =
        json!(NearToken::from_millinear(200).as_yoctonear().to_string());
    fleet
        .council
        .call(registry.id(), "update_fee_config")
        .args_json(json!({ "config": fee }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    Ok(registry)
}

pub async fn rent_price(registry: &Contract, tla: &AccountId, name: &str) -> Result<u128> {
    let price: serde_json::Value = registry
        .view("get_rent_price")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    Ok(price["total_yocto"].as_str().unwrap().parse()?)
}

/// Waits until the minted wallet answers a standard view, which is the
/// readiness signal now that no access key is installed at mint.
pub async fn settled_wallet(worker: &Worker<Sandbox>, id: &AccountId) -> Result<Vec<String>> {
    for _ in 0..15 {
        if let Ok(result) = worker.view(id, "w_extensions").await {
            if let Ok(extensions) = result.json::<Vec<String>>() {
                if !extensions.is_empty() {
                    return Ok(extensions);
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    bail!("{id} never became queryable as a wallet contract")
}

pub async fn owner_key(worker: &Worker<Sandbox>, id: &AccountId) -> Result<String> {
    Ok(worker.view(id, "w_public_key").await?.json::<String>()?)
}

pub async fn owner_account(
    worker: &Worker<Sandbox>,
    id: &AccountId,
    authority: &AccountId,
) -> Result<String> {
    let extensions: Vec<String> = worker.view(id, "w_extensions").await?.json()?;
    Ok(extensions
        .into_iter()
        .find(|held| held != authority.as_str())
        .unwrap_or_default())
}
pub async fn rent(
    fleet: &Fleet,
    registry: &Contract,
    tla: &AccountId,
    name: &str,
) -> Result<AccountId> {
    let total = rent_price(registry, tla, name).await?;
    let out = fleet
        .bob
        .call(registry.id(), "rent_sub_account")
        .args_json(json!({
            "tla_id": tla,
            "name": name,
        }))
        .deposit(NearToken::from_yoctonear(
            total + NearToken::from_millinear(100).as_yoctonear(),
        ))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = out.receipt_failures().first() {
        bail!("rent receipt failed: {failure:?}");
    }
    let tenant: AccountId = format!("{name}.{tla}").parse()?;
    settled_wallet(&fleet.worker, &tenant).await?;
    Ok(tenant)
}
