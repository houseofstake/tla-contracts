mod common;

use anyhow::Result;
use common::*;
use defuse_wallet::actions::FunctionCall;
use defuse_wallet::{NearPromise, Request};
use near_sdk::json_types::U128;
use near_sdk::{Gas as SdkGas, NearToken as SdkToken};
use near_workspaces::result::ExecutionFinalResult;
use near_workspaces::types::{Gas as WsGas, NearToken};
use near_workspaces::{AccountId, Contract};
use serde_json::json;

const RENT: NearToken = NearToken::from_near(5);
const DEV_BALANCE: NearToken = NearToken::from_near(5);
const ONE_YOCTO: SdkToken = SdkToken::from_yoctonear(1);
const NO_DEPOSIT: SdkToken = SdkToken::from_yoctonear(0);

struct Tenant {
    fleet: Fleet,
    id: AccountId,
}

impl Tenant {
    /// The owner account drives the leased account through the standard
    /// extension entrypoint.
    async fn execute(&self, out: NearPromise, tgas: u64) -> Result<ExecutionFinalResult> {
        Ok(self
            .fleet
            .bob
            .call(&self.id, "w_execute_extension")
            .args_json(json!({ "request": Request::new().external([out]) }))
            .deposit(NearToken::from_yoctonear(1))
            .gas(WsGas::from_tgas(tgas))
            .transact()
            .await?)
    }

    async fn balance(&self) -> Result<u128> {
        balance_of(&self.fleet.worker, &self.id).await
    }
}

async fn tenant() -> Result<Tenant> {
    let fleet = deploy_fleet().await?;
    let id = mint(&fleet, "renter", RENT).await?;
    Ok(Tenant { fleet, id })
}

fn sdk_id(id: &AccountId) -> near_sdk::AccountId {
    id.as_str().parse().unwrap()
}

fn send(to: &AccountId, amount: SdkToken) -> NearPromise {
    NearPromise::new(sdk_id(to)).transfer(amount)
}

fn invoke(
    to: &AccountId,
    method: &str,
    args: serde_json::Value,
    gas: SdkGas,
    deposit: SdkToken,
) -> NearPromise {
    NearPromise::new(sdk_id(to)).function_call(call_action(method, args, gas, deposit))
}

fn call_action(
    method: &str,
    args: serde_json::Value,
    gas: SdkGas,
    deposit: SdkToken,
) -> FunctionCall {
    FunctionCall::name(method)
        .args(serde_json::to_vec(&args).unwrap())
        .gas(gas)
        .attach_deposit(deposit)
}

async fn deploy_dev(fleet: &Fleet, name: &str, artifact: &str) -> Result<Contract> {
    let account = fleet
        .relay
        .create_subaccount(name)
        .initial_balance(DEV_BALANCE)
        .transact()
        .await?
        .into_result()?;
    Ok(account.deploy(&wasm(artifact)).await?.into_result()?)
}

async fn deploy_pool(fleet: &Fleet) -> Result<Contract> {
    let pool = deploy_dev(fleet, "pool", "test_staking_pool").await?;
    pool.call("new").transact().await?.into_result()?;
    Ok(pool)
}

async fn deploy_ft(fleet: &Fleet) -> Result<Contract> {
    let ft = deploy_dev(fleet, "ft", "test_ft").await?;
    ft.call("new")
        .args_json(json!({ "owner": ft.id(), "total_supply": U128(1_000_000) }))
        .transact()
        .await?
        .into_result()?;
    Ok(ft)
}

async fn deploy_dapp(fleet: &Fleet, token: &AccountId) -> Result<Contract> {
    let dapp = deploy_dev(fleet, "dapp", "test_dapp").await?;
    dapp.call("new")
        .args_json(json!({ "token": token }))
        .transact()
        .await?
        .into_result()?;
    Ok(dapp)
}

async fn ft_balance(ft: &Contract, who: &AccountId) -> Result<u128> {
    let value: U128 = ft
        .view("ft_balance_of")
        .args_json(json!({ "account_id": who }))
        .await?
        .json()?;
    Ok(value.0)
}

async fn ft_register(ft: &Contract, who: &AccountId) -> Result<()> {
    ft.call("mint")
        .args_json(json!({ "account_id": who, "amount": U128(0) }))
        .transact()
        .await?
        .into_result()?;
    Ok(())
}

#[tokio::test]
async fn near_arrives_and_leaves_like_any_wallet() -> Result<()> {
    let t = tenant().await?;
    let before_in = t.balance().await?;
    t.fleet
        .bob
        .transfer_near(&t.id, NearToken::from_near(1))
        .await?
        .into_result()?;
    let after_in = t.balance().await?;
    assert!(
        after_in > before_in,
        "inbound transfer did not credit the account"
    );

    let bob_before = balance_of(&t.fleet.worker, t.fleet.bob.id()).await?;
    let outcome = t
        .execute(send(t.fleet.bob.id(), SdkToken::from_near(2)), 30)
        .await?;
    assert!(
        outcome.is_success(),
        "outbound transfer failed: {outcome:#?}"
    );
    let bob_after = balance_of(&t.fleet.worker, t.fleet.bob.id()).await?;

    println!("RECEIVE NEAR : +{} yocto credited", after_in - before_in);
    println!("SEND NEAR    : bob +{} yocto", bob_after - bob_before);
    Ok(())
}

#[tokio::test]
async fn user_stakes_into_a_pool_of_their_own_choosing() -> Result<()> {
    let t = tenant().await?;
    let pool = deploy_pool(&t.fleet).await?;
    let stake = SdkToken::from_near(2);

    let outcome = t
        .execute(
            invoke(
                pool.id(),
                "deposit_and_stake",
                json!({}),
                SdkGas::from_tgas(20),
                stake,
            ),
            40,
        )
        .await?;
    assert!(
        outcome.is_success(),
        "deposit_and_stake failed: {outcome:#?}"
    );
    let staked: U128 = pool
        .view("get_account_staked_balance")
        .args_json(json!({ "account_id": t.id }))
        .await?
        .json()?;
    assert_eq!(staked.0, stake.as_yoctonear(), "stake was not credited");

    let amount = json!({ "amount": U128(stake.as_yoctonear()) });
    let outcome = t
        .execute(
            invoke(
                pool.id(),
                "unstake",
                amount.clone(),
                SdkGas::from_tgas(20),
                NO_DEPOSIT,
            ),
            40,
        )
        .await?;
    assert!(outcome.is_success(), "unstake failed: {outcome:#?}");

    let before = t.balance().await?;
    let outcome = t
        .execute(
            invoke(
                pool.id(),
                "withdraw",
                amount,
                SdkGas::from_tgas(20),
                NO_DEPOSIT,
            ),
            40,
        )
        .await?;
    assert!(outcome.is_success(), "withdraw failed: {outcome:#?}");
    let after = t.balance().await?;
    assert!(after > before, "withdrawn stake did not return");

    println!("STAKE        : 2 NEAR delegated to {}", pool.id());
    println!("UNSTAKE      : returned +{} yocto", after - before);
    Ok(())
}

#[tokio::test]
async fn fungible_tokens_are_held_and_moved() -> Result<()> {
    let t = tenant().await?;
    let ft = deploy_ft(&t.fleet).await?;

    let outcome = t
        .execute(
            invoke(
                ft.id(),
                "storage_deposit",
                json!({}),
                SdkGas::from_tgas(15),
                SdkToken::from_yoctonear(hos_common::FT_STORAGE_DEPOSIT_YOCTO),
            ),
            35,
        )
        .await?;
    assert!(outcome.is_success(), "storage_deposit failed: {outcome:#?}");

    ft.call("ft_transfer")
        .args_json(json!({ "receiver_id": t.id, "amount": U128(1_000) }))
        .deposit(NearToken::from_yoctonear(1))
        .transact()
        .await?
        .into_result()?;
    assert_eq!(
        ft_balance(&ft, &t.id).await?,
        1_000,
        "tokens did not arrive"
    );

    ft_register(&ft, t.fleet.bob.id()).await?;
    let outcome = t
        .execute(
            invoke(
                ft.id(),
                "ft_transfer",
                json!({ "receiver_id": t.fleet.bob.id(), "amount": U128(400) }),
                SdkGas::from_tgas(15),
                ONE_YOCTO,
            ),
            35,
        )
        .await?;
    assert!(outcome.is_success(), "ft_transfer failed: {outcome:#?}");
    assert_eq!(ft_balance(&ft, &t.id).await?, 600);
    assert_eq!(ft_balance(&ft, t.fleet.bob.id()).await?, 400);

    println!("RECEIVE FT   : 1000 units credited to {}", t.id);
    println!("SEND FT      : 400 units to bob, 600 retained");
    Ok(())
}

#[tokio::test]
async fn user_enters_a_defi_position_with_ft_transfer_call() -> Result<()> {
    let t = tenant().await?;
    let ft = deploy_ft(&t.fleet).await?;
    let dapp = deploy_dapp(&t.fleet, ft.id()).await?;
    ft_register(&ft, dapp.id()).await?;
    ft.call("mint")
        .args_json(json!({ "account_id": t.id, "amount": U128(5_000) }))
        .transact()
        .await?
        .into_result()?;

    let outcome = t
        .execute(
            invoke(
                ft.id(),
                "ft_transfer_call",
                json!({ "receiver_id": dapp.id(), "amount": U128(5_000), "msg": "deposit" }),
                SdkGas::from_tgas(90),
                ONE_YOCTO,
            ),
            110,
        )
        .await?;
    assert!(
        outcome.is_success(),
        "ft_transfer_call failed: {outcome:#?}"
    );

    let position: U128 = dapp
        .view("get_position")
        .args_json(json!({ "account_id": t.id }))
        .await?
        .json()?;
    assert_eq!(position.0, 5_000, "the dapp did not credit the position");
    assert_eq!(ft_balance(&ft, &t.id).await?, 0);
    assert_eq!(ft_balance(&ft, dapp.id()).await?, 5_000);

    println!("DEFI ENTRY   : 5000 units deposited into {}", dapp.id());
    println!("DEFI POSITION: {} units credited to {}", position.0, t.id);
    Ok(())
}

#[tokio::test]
async fn ft_transfer_call_into_the_wallet_matches_a_plain_account() -> Result<()> {
    let t = tenant().await?;
    let ft = deploy_ft(&t.fleet).await?;
    ft_register(&ft, &t.id).await?;
    ft_register(&ft, t.fleet.bob.id()).await?;

    for target in [t.id.clone(), t.fleet.bob.id().clone()] {
        let outcome = ft
            .call("ft_transfer_call")
            .args_json(json!({ "receiver_id": target, "amount": U128(700), "msg": "" }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await?;
        assert!(
            outcome.is_success(),
            "ft_transfer_call failed: {outcome:#?}"
        );
        let landed = ft_balance(&ft, &target).await?;
        assert_eq!(
            landed, 0,
            "{target} unexpectedly kept ft_transfer_call funds"
        );
        println!("FT_TRANSFER_CALL into {target}: refunded to sender (balance {landed})");
    }

    println!("PARITY       : the tenant wallet behaves exactly like a plain NEAR account here");
    Ok(())
}

#[tokio::test]
async fn the_account_carries_no_key_and_the_owner_account_drives_it() -> Result<()> {
    let t = tenant().await?;
    let keys = t.fleet.worker.view_access_keys(&t.id).await?;
    assert!(
        keys.is_empty(),
        "a leased account should carry no access key, found {keys:?}"
    );
    let public_key: String = t.fleet.worker.view(&t.id, "w_public_key").await?.json()?;
    let signature_allowed: bool = t
        .fleet
        .worker
        .view(&t.id, "w_is_signature_allowed")
        .await?
        .json()?;
    assert_eq!(public_key, "", "the account must report no key");
    assert!(!signature_allowed, "signatures must be refused outright");

    let bob_before = balance_of(&t.fleet.worker, t.fleet.bob.id()).await?;
    let outcome = t
        .execute(send(t.fleet.bob.id(), SdkToken::from_near(1)), 300)
        .await?;
    assert!(
        outcome.is_success(),
        "the owner account could not drive the lease: {outcome:#?}"
    );
    assert!(
        balance_of(&t.fleet.worker, t.fleet.bob.id()).await? > bob_before,
        "the transfer did not land"
    );

    println!("NO KEY       : zero access keys, w_public_key is empty");
    println!("OWNER PATH   : the owner AccountId drove a 300 TGas transaction");
    Ok(())
}

#[tokio::test]
async fn a_signed_request_is_refused_whoever_signs_it() -> Result<()> {
    let t = tenant().await?;
    let msg = json!({
        "chain_id": t.fleet.worker.status().await?.chain_id,
        "signer_id": t.id,
        "nonce": 1,
        "created_at": "2020-01-01T00:00:00Z",
        "timeout_secs": 3600,
        "request": { "external": [] },
    });
    let outcome = t
        .fleet
        .relay
        .call(&t.id, "w_execute_signed")
        .args_json(json!({ "msg": msg, "proof": "ed25519:anything" }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(WsGas::from_tgas(60))
        .transact()
        .await?;
    assert!(
        outcome.is_failure(),
        "the signed path must be closed: {outcome:#?}"
    );

    println!("SIGNED PATH  : permanently CLOSED, no key can ever drive this account");
    Ok(())
}

#[tokio::test]
async fn a_co_owner_can_be_granted_and_can_act() -> Result<()> {
    let t = tenant().await?;
    let carol = t
        .fleet
        .relay
        .create_subaccount("carol")
        .initial_balance(NearToken::from_near(3))
        .transact()
        .await?
        .into_result()?;

    let grant = t
        .fleet
        .bob
        .call(&t.id, "w_execute_extension")
        .args_json(json!({ "request": {
            "internal": [{ "op": "add_extension", "payload": { "account_id": carol.id() } }]
        }}))
        .deposit(NearToken::from_yoctonear(1))
        .gas(WsGas::from_tgas(40))
        .transact()
        .await?;
    assert!(grant.is_success(), "granting a co-owner failed: {grant:#?}");

    let enabled: bool = t
        .fleet
        .worker
        .view(&t.id, "w_is_extension_enabled")
        .args_json(json!({ "account_id": carol.id() }))
        .await?
        .json()?;
    assert!(enabled, "the co-owner was not enabled");

    let ungranted = carol
        .call(&t.id, "w_execute_extension")
        .args_json(json!({ "request": Request::new().external([send(t.fleet.bob.id(), SdkToken::from_yoctonear(1))]) }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(WsGas::from_tgas(60))
        .transact()
        .await?;
    assert!(
        ungranted.is_failure(),
        "an extension must not spend before the owner grants a scope: {ungranted:#?}"
    );

    let scoped = t
        .fleet
        .bob
        .call(&t.id, "hos_grant_spend")
        .args_json(json!({
            "extension": carol.id(),
            "receivers": [t.fleet.bob.id()],
            "budget_yocto": "1000000000000000000000",
            "tokens": [],
            "items": [],
            "expires_at": lease_until_ns().to_string(),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(WsGas::from_tgas(40))
        .transact()
        .await?;
    assert!(
        scoped.is_success(),
        "granting a spend scope failed: {scoped:#?}"
    );

    let acted = carol
        .call(&t.id, "w_execute_extension")
        .args_json(json!({ "request": Request::new().external([send(t.fleet.bob.id(), SdkToken::from_yoctonear(1))]) }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(WsGas::from_tgas(60))
        .transact()
        .await?;
    assert!(acted.is_success(), "the co-owner could not act: {acted:#?}");

    println!("CO-OWNER     : granted by AccountId, acted through w_execute_extension");
    Ok(())
}

#[tokio::test]
async fn a_granted_agent_moves_tokens_up_to_its_budget_and_no_further() -> Result<()> {
    let t = tenant().await?;
    let ft = deploy_ft(&t.fleet).await?;
    let agent = t
        .fleet
        .relay
        .create_subaccount("agent")
        .initial_balance(NearToken::from_near(3))
        .transact()
        .await?
        .into_result()?;

    ft.call("mint")
        .args_json(json!({ "account_id": t.id, "amount": U128(1_000) }))
        .transact()
        .await?
        .into_result()?;
    ft_register(&ft, t.fleet.bob.id()).await?;

    t.fleet
        .bob
        .call(&t.id, "w_execute_extension")
        .args_json(json!({ "request": {
            "internal": [{ "op": "add_extension", "payload": { "account_id": agent.id() } }]
        }}))
        .deposit(NearToken::from_yoctonear(1))
        .gas(WsGas::from_tgas(40))
        .transact()
        .await?
        .into_result()?;

    t.fleet
        .bob
        .call(&t.id, "hos_grant_spend")
        .args_json(json!({
            "extension": agent.id(),
            "receivers": [t.fleet.bob.id()],
            "budget_yocto": "0",
            "tokens": [{ "token": ft.id(), "budget": U128(500) }],
            "items": [],
            "expires_at": lease_until_ns().to_string(),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(WsGas::from_tgas(40))
        .transact()
        .await?
        .into_result()?;

    let pay = |amount: u128| {
        json!({ "request": Request::new().external([invoke(
            ft.id(),
            "ft_transfer",
            json!({ "receiver_id": t.fleet.bob.id(), "amount": U128(amount) }),
            SdkGas::from_tgas(15),
            ONE_YOCTO,
        )]) })
    };

    let within = agent
        .call(&t.id, "w_execute_extension")
        .args_json(pay(400))
        .deposit(NearToken::from_yoctonear(1))
        .gas(WsGas::from_tgas(80))
        .transact()
        .await?;
    assert!(
        within.is_success(),
        "an agent must be able to pay in tokens: {within:#?}"
    );
    assert_eq!(ft_balance(&ft, t.fleet.bob.id()).await?, 400);

    let over = agent
        .call(&t.id, "w_execute_extension")
        .args_json(pay(200))
        .deposit(NearToken::from_yoctonear(1))
        .gas(WsGas::from_tgas(80))
        .transact()
        .await?;
    assert!(
        over.is_failure(),
        "the token budget must bound the amount inside the arguments: {over:#?}"
    );
    assert_eq!(
        ft_balance(&ft, t.fleet.bob.id()).await?,
        400,
        "a refused spend must move nothing"
    );

    println!("AGENT TOKENS : 400 of a 500 budget spent, the next 200 refused");
    Ok(())
}

#[tokio::test]
async fn the_owner_can_act_repeatedly_without_degrading() -> Result<()> {
    let t = tenant().await?;
    for _ in 0..5 {
        let outcome = t
            .execute(send(t.fleet.bob.id(), SdkToken::from_yoctonear(1)), 30)
            .await?;
        assert!(outcome.is_success(), "transfer failed: {outcome:#?}");
    }

    println!("REPEATED     : five consecutive owner-driven transfers all succeeded");
    Ok(())
}

#[tokio::test]
async fn spending_cannot_drain_the_storage_reserve() -> Result<()> {
    let t = tenant().await?;
    let balance = t.balance().await?;
    let outcome = t
        .execute(
            send(t.fleet.bob.id(), SdkToken::from_yoctonear(balance)),
            60,
        )
        .await?;
    assert!(
        outcome.is_failure(),
        "a transfer of the entire balance should breach the reserve: {outcome:#?}"
    );

    println!("RESERVE      : whole-balance transfer REFUSED, account stays fundable");
    Ok(())
}

/// The relay chain the product actually runs: a relay account owns a
/// provider wallet, that provider wallet owns the user's leased name, and the
/// relay drives both hops through the standard extension entrypoint. No key
/// exists at any level.
#[tokio::test]
async fn a_relay_drives_a_users_lease_through_the_provider_account_it_owns() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let relay_id = fleet.relay.id().clone();

    let provider = mint_owned_by(
        &fleet,
        "provider",
        &relay_id,
        NearToken::from_near(10),
        lease_until_ns(),
    )
    .await?;
    let leased = mint_owned_by(
        &fleet,
        "usersname",
        &provider,
        NearToken::from_near(10),
        lease_until_ns(),
    )
    .await?;

    let provider_extensions: Vec<String> =
        fleet.worker.view(&provider, "w_extensions").await?.json()?;
    let leased_extensions: Vec<String> =
        fleet.worker.view(&leased, "w_extensions").await?.json()?;
    assert!(
        provider_extensions.iter().any(|e| e == relay_id.as_str()),
        "the relay must own the provider wallet, got {provider_extensions:?}"
    );
    assert!(
        leased_extensions.iter().any(|e| e == provider.as_str()),
        "the provider wallet must own the leased name, got {leased_extensions:?}"
    );

    let bob_before = balance_of(&fleet.worker, fleet.bob.id()).await?;

    // inner hop: the leased name pays bob
    let inner = json!({ "request": spend_request(fleet.bob.id(), SdkToken::from_near(1)) });
    // outer hop: the provider wallet calls the leased name
    let outer = NearPromise::new(sdk_id(&leased)).function_call(
        FunctionCall::name("w_execute_extension")
            .args(serde_json::to_vec(&inner)?)
            .gas(SdkGas::from_tgas(120))
            .attach_deposit(ONE_YOCTO),
    );

    let outcome = fleet
        .relay
        .call(&provider, "w_execute_extension")
        .args_json(json!({ "request": Request::new().external([outer]) }))
        .deposit(NearToken::from_yoctonear(1))
        .gas(WsGas::from_tgas(250))
        .transact()
        .await?;
    assert!(
        outcome.is_success(),
        "the relay two-hop failed: {outcome:#?}"
    );
    if let Some(failure) = outcome.receipt_failures().first() {
        panic!("a receipt in the two-hop chain failed: {failure:?}");
    }

    assert!(
        balance_of(&fleet.worker, fleet.bob.id()).await? > bob_before,
        "the leased name did not pay out through the provider wallet"
    );

    println!("RELAY CHAIN: relay -> provider wallet -> leased name, zero keys involved");
    Ok(())
}
