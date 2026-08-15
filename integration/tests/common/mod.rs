#![allow(dead_code)]

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use defuse_wallet::{NearPromise, Request};
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
    let relay = subaccount(&root, "relay", NearToken::from_near(80)).await?;
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
        .args_json(json!({ "code": wallet_wasm }))
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
