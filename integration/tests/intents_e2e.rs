mod common;

use anyhow::{bail, Result};
use common::*;
use defuse_core::nep413::{Nep413, Nep413Payload};
use ed25519_dalek::{Signer, SigningKey};
use near_sdk::json_types::U128;
use near_workspaces::types::NearToken;
use near_workspaces::{Account, Contract};
use serde_json::json;

struct Signing {
    key: SigningKey,
    account: Account,
}

impl Signing {
    fn new(account: Account) -> Self {
        Self {
            key: SigningKey::generate(&mut rand::rngs::OsRng),
            account,
        }
    }

    fn public_key(&self) -> String {
        format!(
            "ed25519:{}",
            bs58::encode(self.key.verifying_key().to_bytes()).into_string()
        )
    }

    async fn register(&self, verifier: &Contract) -> Result<()> {
        self.account
            .call(verifier.id(), "add_public_key")
            .args_json(json!({ "public_key": self.public_key() }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        Ok(())
    }

    fn sign(
        &self,
        verifier: &Contract,
        nonce: [u8; 32],
        body: serde_json::Value,
    ) -> serde_json::Value {
        let payload = Nep413Payload::new(body.to_string())
            .recipient(verifier.id().as_str())
            .nonce(nonce);
        let signature = self.key.sign(&Nep413::prehash(&payload));
        json!({
            "standard": "nep413",
            "payload": {
                "message": payload.message,
                "nonce": base64_of(&payload.nonce),
                "recipient": payload.recipient,
            },
            "public_key": self.public_key(),
            "signature": format!("ed25519:{}", bs58::encode(signature.to_bytes()).into_string()),
        })
    }
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Mirrors the protocol's own `create_random_salted_nonce`: magic prefix, version,
/// the salt the verifier is currently issuing, the deadline in nanoseconds, and
/// fifteen random bytes.
fn versioned_nonce(salt_hex: &str, deadline_nanos: i64) -> [u8; 32] {
    let mut nonce = [0u8; 32];
    nonce[..4].copy_from_slice(&[0x56, 0x28, 0xf6, 0xc6]);
    nonce[4] = 0;
    let salt = (0..4)
        .map(|i| u8::from_str_radix(&salt_hex[i * 2..i * 2 + 2], 16).expect("salt is hex"))
        .collect::<Vec<_>>();
    nonce[5..9].copy_from_slice(&salt);
    nonce[9..17].copy_from_slice(&deadline_nanos.to_le_bytes());
    for slot in nonce.iter_mut().skip(17) {
        *slot = rand::random();
    }
    nonce
}

fn legacy_nonce() -> [u8; 32] {
    let mut nonce = [0u8; 32];
    for slot in nonce.iter_mut() {
        *slot = rand::random();
    }
    nonce
}

/// mainnet issues salts and takes versioned nonces; the pinned release artifact
/// predates that and only understands the legacy form. Asking the verifier which
/// it is keeps one harness honest against both.
async fn nonce_for(verifier: &Contract, deadline_nanos: i64) -> [u8; 32] {
    match verifier.view("current_salt").await {
        Ok(result) => match result.json::<String>() {
            Ok(salt) => versioned_nonce(&salt, deadline_nanos),
            Err(_) => legacy_nonce(),
        },
        Err(_) => legacy_nonce(),
    }
}

fn defuse_wasm() -> Vec<u8> {
    let path = std::env::var("DEFUSE_WASM")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("CARGO_HOME")
                .unwrap_or_else(|_| format!("{}/.cargo", std::env::var("HOME").expect("HOME")));
            let checkouts = std::path::Path::new(&home).join("git").join("checkouts");
            let mut newest = None;
            let roots = std::fs::read_dir(&checkouts).expect("cargo git checkouts must exist");
            for root in roots.flatten() {
                let name = root.file_name();
                if !name.to_string_lossy().starts_with("intents-") {
                    continue;
                }
                let Ok(revs) = std::fs::read_dir(root.path()) else {
                    continue;
                };
                for rev in revs.flatten() {
                    let Ok(files) = std::fs::read_dir(rev.path().join("releases")) else {
                        continue;
                    };
                    for file in files.flatten() {
                        let candidate = file.path();
                        let file_name = candidate.file_name().unwrap_or_default().to_string_lossy();
                        if file_name.starts_with("defuse-") && file_name.ends_with(".wasm") {
                            newest = Some(candidate);
                        }
                    }
                }
            }
            newest.expect("no defuse release wasm in the vendored intents checkout")
        });
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"))
}

async fn funded(fleet: &Fleet, name: &str, near: u128) -> Result<Account> {
    Ok(fleet
        .worker
        .root_account()?
        .create_subaccount(name)
        .initial_balance(NearToken::from_near(near))
        .transact()
        .await?
        .into_result()?)
}

async fn deploy_verifier(fleet: &Fleet, fee_pips: u32) -> Result<Contract> {
    let account = funded(fleet, "intents", 30).await?;
    let verifier = account.deploy(&defuse_wasm()).await?.into_result()?;
    verifier
        .call("new")
        .args_json(json!({ "config": {
            "wnear_id": "wrap.test.near",
            "fees": { "fee": fee_pips, "fee_collector": fleet.council.id() },
            "roles": { "super_admins": [fleet.council.id()], "admins": {}, "grantees": {} },
        }}))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    Ok(verifier)
}

async fn held_amount(verifier: &Contract, holder: &Account, token: &str) -> Result<u128> {
    let raw: String = verifier
        .view("mt_balance_of")
        .args_json(json!({ "account_id": holder.id(), "token_id": token }))
        .await?
        .json()?;
    Ok(raw.parse()?)
}

#[tokio::test]
async fn a_leased_name_can_be_deposited_into_intents() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let verifier = deploy_verifier(&fleet, 0).await?;

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    let token_id = format!("{name}.{tla}");
    let defuse_token = format!("nep171:{}:{token_id}", registry.id());

    let deposited = fleet
        .bob
        .call(registry.id(), "nft_transfer_call")
        .args_json(json!({
            "receiver_id": verifier.id(),
            "token_id": token_id,
            "approval_id": null,
            "memo": null,
            "msg": json!({ "receiver_id": fleet.bob.id() }).to_string(),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = deposited.receipt_failures().first() {
        bail!("intents refused the deposit: {failure:?}");
    }

    assert_eq!(
        held_amount(&verifier, &fleet.bob, &defuse_token).await?,
        1,
        "the seller must hold exactly one of the name inside the verifier"
    );
    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        verifier.id().as_str(),
        "and the account itself must now be held by intents"
    );

    Ok(())
}

async fn deploy_ft(fleet: &Fleet) -> Result<Contract> {
    let account = funded(fleet, "ft", 10).await?;
    let ft = account.deploy(&wasm("test_ft")).await?.into_result()?;
    ft.call("new")
        .args_json(json!({ "owner": ft.id(), "total_supply": U128(1_000_000_000) }))
        .transact()
        .await?
        .into_result()?;
    Ok(ft)
}

async fn fund_inside_intents(
    ft: &Contract,
    verifier: &Contract,
    who: &Account,
    amount: u128,
) -> Result<()> {
    for account in [who.id(), verifier.id()] {
        ft.call("storage_deposit")
            .args_json(json!({ "account_id": account }))
            .deposit(NearToken::from_millinear(10))
            .transact()
            .await?
            .into_result()?;
    }
    ft.call("mint")
        .args_json(json!({ "account_id": who.id(), "amount": U128(amount) }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    who.call(ft.id(), "ft_transfer_call")
        .args_json(json!({
            "receiver_id": verifier.id(),
            "amount": U128(amount),
            "memo": null,
            "msg": json!({ "receiver_id": who.id() }).to_string(),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    Ok(())
}

#[tokio::test]
async fn a_name_settles_against_tokens_and_leaves_with_the_buyer() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let verifier = deploy_verifier(&fleet, 0).await?;
    let ft = deploy_ft(&fleet).await?;

    let name = "alice";
    let tenant = rent(&fleet, &registry, &tla, name).await?;
    let token_id = format!("{name}.{tla}");
    let name_token = format!("nep171:{}:{token_id}", registry.id());
    let cash_token = format!("nep141:{}", ft.id());
    let price = 500_000u128;

    let buyer = funded(&fleet, "buyer", 20).await?;

    fleet
        .bob
        .call(registry.id(), "nft_transfer_call")
        .args_json(json!({
            "receiver_id": verifier.id(),
            "token_id": token_id,
            "approval_id": null,
            "memo": null,
            "msg": json!({ "receiver_id": fleet.bob.id() }).to_string(),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    fund_inside_intents(&ft, &verifier, &buyer, price).await?;

    let seller = Signing::new(fleet.bob.clone());
    let purchaser = Signing::new(buyer.clone());
    seller.register(&verifier).await?;
    purchaser.register(&verifier).await?;

    let deadline_nanos = (chain_timestamp_ns(&fleet.worker).await? + 600_000_000_000) as i64;
    let deadline = "2100-01-01T00:00:00.000Z";

    let seller_payload = seller.sign(
        &verifier,
        nonce_for(&verifier, deadline_nanos).await,
        json!({
            "signer_id": fleet.bob.id(),
            "deadline": deadline,
            "intents": [{
                "intent": "token_diff",
                "diff": { name_token.clone(): "-1", cash_token.clone(): price.to_string() },
            }],
        }),
    );
    let buyer_payload = purchaser.sign(
        &verifier,
        nonce_for(&verifier, deadline_nanos).await,
        json!({
            "signer_id": buyer.id(),
            "deadline": deadline,
            "intents": [{
                "intent": "token_diff",
                "diff": { name_token.clone(): "1", cash_token.clone(): format!("-{price}") },
            }],
        }),
    );

    let simulated: serde_json::Value = verifier
        .view("simulate_intents")
        .args_json(json!({ "signed": [seller_payload, buyer_payload] }))
        .await?
        .json()?;
    assert!(
        simulated
            .get("invariant_violated")
            .is_none_or(|v| v.is_null()),
        "the pair must net before we spend gas on it: {simulated}"
    );

    let executed = fleet
        .relay
        .call(verifier.id(), "execute_intents")
        .args_json(json!({ "signed": [seller_payload, buyer_payload] }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = executed.receipt_failures().first() {
        bail!("settlement failed: {failure:?}");
    }

    assert_eq!(
        held_amount(&verifier, &buyer, &name_token).await?,
        1,
        "the buyer must hold the name inside the verifier"
    );
    assert_eq!(
        held_amount(&verifier, &fleet.bob, &cash_token).await?,
        price,
        "and the seller must hold the price"
    );

    let withdrawal = purchaser.sign(
        &verifier,
        nonce_for(&verifier, deadline_nanos).await,
        json!({
            "signer_id": buyer.id(),
            "deadline": deadline,
            "intents": [{
                "intent": "nft_withdraw",
                "token": registry.id(),
                "receiver_id": buyer.id(),
                "token_id": token_id,
            }],
        }),
    );
    let withdrawn = fleet
        .relay
        .call(verifier.id(), "execute_intents")
        .args_json(json!({ "signed": [withdrawal] }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = withdrawn.receipt_failures().first() {
        bail!("withdrawal failed: {failure:?}");
    }

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        buyer.id().as_str(),
        "the leased account itself must end up under the buyer"
    );
    let sub: serde_json::Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": name }))
        .await?
        .json()?;
    assert_eq!(
        sub["owner"],
        buyer.id().as_str(),
        "and the registry index must agree"
    );

    Ok(())
}

struct Market {
    fleet: Fleet,
    registry: Contract,
    verifier: Contract,
    ft: Contract,
    tenant: near_workspaces::AccountId,
    token_id: String,
    name_token: String,
    cash_token: String,
    seller: Signing,
    buyer_account: Account,
    buyer: Signing,
    deadline_nanos: i64,
}

const PRICE: u128 = 500_000;

async fn open_market(fee_pips: u32) -> Result<Market> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let verifier = deploy_verifier(&fleet, fee_pips).await?;
    fleet
        .council
        .call(registry.id(), "add_venue")
        .args_json(json!({ "account_id": verifier.id() }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    let ft = deploy_ft(&fleet).await?;

    let tenant = rent(&fleet, &registry, &tla, "alice").await?;
    let token_id = format!("alice.{tla}");
    let name_token = format!("nep171:{}:{token_id}", registry.id());
    let cash_token = format!("nep141:{}", ft.id());

    let buyer_account = funded(&fleet, "buyer", 20).await?;
    fund_inside_intents(&ft, &verifier, &buyer_account, PRICE * 4).await?;

    let seller = Signing::new(fleet.bob.clone());
    let buyer = Signing::new(buyer_account.clone());
    seller.register(&verifier).await?;
    buyer.register(&verifier).await?;

    let deadline_nanos = (chain_timestamp_ns(&fleet.worker).await? + 600_000_000_000) as i64;

    Ok(Market {
        fleet,
        registry,
        verifier,
        ft,
        tenant,
        token_id,
        name_token,
        cash_token,
        seller,
        buyer_account,
        buyer,
        deadline_nanos,
    })
}

impl Market {
    async fn deposit_the_name(&self) -> Result<()> {
        self.fleet
            .bob
            .call(self.registry.id(), "nft_transfer_call")
            .args_json(json!({
                "receiver_id": self.verifier.id(),
                "token_id": self.token_id,
                "approval_id": null,
                "memo": null,
                "msg": json!({ "receiver_id": self.fleet.bob.id() }).to_string(),
            }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await?
            .into_result()?;
        Ok(())
    }

    async fn nonce(&self) -> [u8; 32] {
        nonce_for(&self.verifier, self.deadline_nanos).await
    }

    fn far_future(&self) -> &'static str {
        "2100-01-01T00:00:00.000Z"
    }

    async fn sell_side(&self, cash_in: i128) -> serde_json::Value {
        self.seller.sign(
            &self.verifier,
            self.nonce().await,
            json!({
                "signer_id": self.fleet.bob.id(),
                "deadline": self.far_future(),
                "intents": [{ "intent": "token_diff", "diff": {
                    self.name_token.clone(): "-1",
                    self.cash_token.clone(): cash_in.to_string(),
                }}],
            }),
        )
    }

    async fn buy_side(&self, name_in: i128) -> serde_json::Value {
        self.buyer.sign(
            &self.verifier,
            self.nonce().await,
            json!({
                "signer_id": self.buyer_account.id(),
                "deadline": self.far_future(),
                "intents": [{ "intent": "token_diff", "diff": {
                    self.name_token.clone(): name_in.to_string(),
                    self.cash_token.clone(): format!("-{PRICE}"),
                }}],
            }),
        )
    }

    async fn execute(&self, signed: Vec<serde_json::Value>) -> Result<bool> {
        let outcome = self
            .fleet
            .relay
            .call(self.verifier.id(), "execute_intents")
            .args_json(json!({ "signed": signed }))
            .max_gas()
            .transact()
            .await?;
        Ok(outcome.is_success() && outcome.receipt_failures().is_empty())
    }

    async fn cash_of(&self, who: &Account) -> Result<u128> {
        held_amount(&self.verifier, who, &self.cash_token).await
    }

    async fn name_held_by(&self, who: &Account) -> Result<u128> {
        held_amount(&self.verifier, who, &self.name_token).await
    }
}

fn closure(token: &str, delta: i128, fee_pips: u32) -> i128 {
    use defuse_core::fees::Pips;
    use defuse_core::token_id::TokenId;
    let token_id: TokenId = token.parse().expect("token id parses");
    let fee = Pips::from_pips(fee_pips).expect("fee in range");
    defuse_core::intents::token_diff::TokenDiff::closure_delta(&token_id, delta, fee)
        .expect("closure fits")
}

#[tokio::test]
async fn a_fee_bearing_sale_pays_the_seller_the_closure_amount() -> Result<()> {
    let fee_pips = 10_000;
    let market = open_market(fee_pips).await?;
    market.deposit_the_name().await?;

    let seller_receives = closure(&market.cash_token, -(PRICE as i128), fee_pips);
    let buyer_receives = closure(&market.name_token, -1, fee_pips);
    assert!(
        seller_receives < PRICE as i128,
        "a one percent fee must leave the seller short of the sticker price"
    );
    assert_eq!(
        buyer_receives, 1,
        "the name is fee exempt, so the buyer must receive all of it"
    );

    let landed = market
        .execute(vec![
            market.sell_side(seller_receives).await,
            market.buy_side(buyer_receives).await,
        ])
        .await?;
    assert!(landed, "a fee-closed pair must settle");

    assert_eq!(market.name_held_by(&market.buyer_account).await?, 1);
    assert_eq!(
        market.cash_of(&market.fleet.bob).await? as i128,
        seller_receives,
        "the seller nets the closure amount, not the sticker price"
    );
    assert_eq!(
        market.cash_of(&market.fleet.council).await? as i128,
        PRICE as i128 - seller_receives,
        "and the fee collector takes exactly the difference"
    );

    Ok(())
}

#[tokio::test]
async fn a_settlement_cannot_be_replayed() -> Result<()> {
    let market = open_market(0).await?;
    market.deposit_the_name().await?;

    let pair = vec![
        market.sell_side(PRICE as i128).await,
        market.buy_side(1).await,
    ];
    assert!(
        market.execute(pair.clone()).await?,
        "the first settlement lands"
    );
    assert!(
        !market.execute(pair).await?,
        "replaying the same signed pair must be refused once the nonces are spent"
    );
    assert_eq!(market.name_held_by(&market.buyer_account).await?, 1);
    assert_eq!(market.cash_of(&market.fleet.bob).await?, PRICE);

    Ok(())
}

#[tokio::test]
async fn an_expired_deadline_is_refused() -> Result<()> {
    let market = open_market(0).await?;
    market.deposit_the_name().await?;

    let stale = market.seller.sign(
        &market.verifier,
        market.nonce().await,
        json!({
            "signer_id": market.fleet.bob.id(),
            "deadline": "2020-01-01T00:00:00.000Z",
            "intents": [{ "intent": "token_diff", "diff": {
                market.name_token.clone(): "-1",
                market.cash_token.clone(): PRICE.to_string(),
            }}],
        }),
    );
    assert!(
        !market
            .execute(vec![stale, market.buy_side(1).await])
            .await?,
        "a deadline in the past must not settle"
    );
    assert_eq!(market.name_held_by(&market.buyer_account).await?, 0);

    Ok(())
}

#[tokio::test]
async fn two_buyers_cannot_race_the_same_listing() -> Result<()> {
    let market = open_market(0).await?;
    market.deposit_the_name().await?;

    let listing = market.sell_side(PRICE as i128).await;
    let first = market.buy_side(1).await;
    let second = market.buy_side(1).await;

    assert!(
        market.execute(vec![listing.clone(), first]).await?,
        "the first buyer takes the name"
    );
    assert!(
        !market.execute(vec![listing, second]).await?,
        "the listing nonce is spent, so a second buyer cannot take it too"
    );
    assert_eq!(
        market.name_held_by(&market.buyer_account).await?,
        1,
        "exactly one name exists and one buyer holds it"
    );

    Ok(())
}

#[tokio::test]
async fn a_seller_who_withdraws_the_name_cannot_be_settled_against() -> Result<()> {
    let market = open_market(0).await?;
    market.deposit_the_name().await?;

    let listing = market.sell_side(PRICE as i128).await;

    let withdrawal = market.seller.sign(
        &market.verifier,
        market.nonce().await,
        json!({
            "signer_id": market.fleet.bob.id(),
            "deadline": market.far_future(),
            "intents": [{
                "intent": "nft_withdraw",
                "token": market.registry.id(),
                "receiver_id": market.fleet.bob.id(),
                "token_id": market.token_id,
            }],
        }),
    );
    assert!(
        market.execute(vec![withdrawal]).await?,
        "the seller may withdraw their name while it is unsold"
    );

    let buyer_cash_before = market.cash_of(&market.buyer_account).await?;
    assert!(
        !market
            .execute(vec![listing, market.buy_side(1).await])
            .await?,
        "a listing against a name the seller already withdrew must not settle"
    );
    assert_eq!(
        market.cash_of(&market.buyer_account).await?,
        buyer_cash_before,
        "and the buyer must not be charged for it"
    );
    assert_eq!(
        owner_account(
            &market.fleet.worker,
            &market.tenant,
            market.fleet.extension.id()
        )
        .await?,
        market.fleet.bob.id().as_str(),
        "the name is back with its owner"
    );

    Ok(())
}

#[tokio::test]
async fn a_pair_that_does_not_net_is_refused() -> Result<()> {
    let market = open_market(0).await?;
    market.deposit_the_name().await?;

    let underpaying = market.buyer.sign(
        &market.verifier,
        market.nonce().await,
        json!({
            "signer_id": market.buyer_account.id(),
            "deadline": market.far_future(),
            "intents": [{ "intent": "token_diff", "diff": {
                market.name_token.clone(): "1",
                market.cash_token.clone(): format!("-{}", PRICE / 2),
            }}],
        }),
    );
    let seller = market.sell_side(PRICE as i128).await;

    let simulated: serde_json::Value = market
        .verifier
        .view("simulate_intents")
        .args_json(json!({ "signed": [seller.clone(), underpaying.clone()] }))
        .await?
        .json()?;
    assert!(
        simulated
            .get("invariant_violated")
            .is_some_and(|v| !v.is_null()),
        "simulation must report the shortfall before gas is spent: {simulated}"
    );
    assert!(
        !market.execute(vec![seller, underpaying]).await?,
        "and the settlement itself must be refused"
    );
    assert_eq!(market.name_held_by(&market.buyer_account).await?, 0);

    Ok(())
}

/// The verifier authenticates a withdrawal by predecessor, so a holder exits
/// with one call and no signature. This is the product's exit path.
#[tokio::test]
async fn a_holder_exits_with_one_call_and_no_signature() -> Result<()> {
    let market = open_market(0).await?;
    market.deposit_the_name().await?;
    assert_eq!(market.name_held_by(&market.fleet.bob).await?, 1);

    let withdrawn = market
        .fleet
        .bob
        .call(market.verifier.id(), "nft_withdraw")
        .args_json(json!({
            "token": market.registry.id(),
            "receiver_id": market.fleet.bob.id(),
            "token_id": market.token_id,
            "memo": null,
            "msg": null,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    if let Some(failure) = withdrawn.receipt_failures().first() {
        bail!("a direct withdrawal must not need our help: {failure:?}");
    }

    assert_eq!(
        market.name_held_by(&market.fleet.bob).await?,
        0,
        "the balance inside the verifier is spent"
    );
    assert_eq!(
        owner_account(
            &market.fleet.worker,
            &market.tenant,
            market.fleet.extension.id()
        )
        .await?,
        market.fleet.bob.id().as_str(),
        "and the leased account is back under its holder"
    );
    Ok(())
}

#[tokio::test]
async fn a_deposited_name_keeps_its_payout_with_the_seller() -> Result<()> {
    let market = open_market(0).await?;
    market.deposit_the_name().await?;

    assert_eq!(
        owner_account(
            &market.fleet.worker,
            &market.tenant,
            market.fleet.extension.id()
        )
        .await?,
        market.verifier.id().as_str(),
        "the verifier holds the name while it is on the book"
    );
    let payout: String = market
        .fleet
        .worker
        .view(&market.tenant, "hos_payout_account")
        .await?
        .json()?;
    assert_eq!(
        payout,
        market.fleet.bob.id().as_str(),
        "but it never earns the balance, or a later sweep pays a contract no one can withdraw from"
    );
    Ok(())
}

#[tokio::test]
async fn a_plain_transfer_into_a_venue_also_keeps_the_payout_with_the_seller() -> Result<()> {
    let market = open_market(0).await?;
    market
        .fleet
        .bob
        .call(market.registry.id(), "nft_transfer")
        .args_json(json!({
            "receiver_id": market.verifier.id(),
            "token_id": market.token_id,
            "approval_id": null,
            "memo": null,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let payout: String = market
        .fleet
        .worker
        .view(&market.tenant, "hos_payout_account")
        .await?
        .json()?;
    assert_eq!(
        payout,
        market.fleet.bob.id().as_str(),
        "reaching a venue by plain transfer rather than transfer_call must not hand the seller's \
         payout claim to the venue, or the proceeds land somewhere the seller cannot withdraw from"
    );

    let sub: serde_json::Value = market
        .registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": market.fleet.registrar.id(), "name": "alice" }))
        .await?
        .json()?;
    assert_eq!(
        sub["payout_account"],
        market.fleet.bob.id().as_str(),
        "and the registry's own record must agree with the wallet"
    );
    Ok(())
}

#[tokio::test]
async fn an_unlisted_receiver_does_not_get_venue_treatment() -> Result<()> {
    let market = open_market(0).await?;
    market
        .fleet
        .council
        .call(market.registry.id(), "remove_venue")
        .args_json(json!({ "account_id": market.verifier.id() }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    market.deposit_the_name().await?;

    let payout: String = market
        .fleet
        .worker
        .view(&market.tenant, "hos_payout_account")
        .await?
        .json()?;
    assert_eq!(
        payout,
        market.verifier.id().as_str(),
        "a receiver the council never named is a plain new owner, not a venue"
    );
    Ok(())
}

#[tokio::test]
async fn a_stranger_cannot_withdraw_a_name_they_do_not_hold() -> Result<()> {
    let market = open_market(0).await?;
    market.deposit_the_name().await?;

    let mallory = funded(&market.fleet, "mallory", 10).await?;
    let attempt = mallory
        .call(market.verifier.id(), "nft_withdraw")
        .args_json(json!({
            "token": market.registry.id(),
            "receiver_id": mallory.id(),
            "token_id": market.token_id,
            "memo": null,
            "msg": null,
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    assert!(
        !attempt.is_success() || !attempt.receipt_failures().is_empty(),
        "a stranger must not be able to withdraw a name they never deposited"
    );
    assert_eq!(
        market.name_held_by(&market.fleet.bob).await?,
        1,
        "and the holder's balance is untouched"
    );
    assert_eq!(
        owner_account(
            &market.fleet.worker,
            &market.tenant,
            market.fleet.extension.id()
        )
        .await?,
        market.verifier.id().as_str(),
        "the name stays inside the verifier"
    );
    Ok(())
}

async fn mint_ft(ft: &Contract, who: &Account, amount: u128) -> Result<()> {
    ft.call("storage_deposit")
        .args_json(json!({ "account_id": who.id() }))
        .deposit(NearToken::from_millinear(10))
        .transact()
        .await?
        .into_result()?;
    ft.call("mint")
        .args_json(json!({ "account_id": who.id(), "amount": U128(amount) }))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    Ok(())
}

impl Market {
    async fn buy_in_one_call(
        &self,
        buyer: &Account,
        amount: u128,
        signed: Vec<serde_json::Value>,
    ) -> Result<bool> {
        let msg = json!({
            "receiver_id": buyer.id(),
            "execute_intents": signed,
            "refund_if_fails": true,
        })
        .to_string();
        let outcome = buyer
            .call(self.ft.id(), "ft_transfer_call")
            .args_json(json!({
                "receiver_id": self.verifier.id(),
                "amount": U128(amount),
                "memo": null,
                "msg": msg,
            }))
            .deposit(NearToken::from_yoctonear(1))
            .max_gas()
            .transact()
            .await?;
        Ok(outcome.is_success() && outcome.receipt_failures().is_empty())
    }

    async fn wallet_ft(&self, who: &Account) -> Result<u128> {
        let raw: U128 = self
            .ft
            .view("ft_balance_of")
            .args_json(json!({ "account_id": who.id() }))
            .await?
            .json()?;
        Ok(raw.0)
    }
}

#[tokio::test]
async fn a_buyer_funds_and_settles_in_one_call() -> Result<()> {
    let market = open_market(0).await?;
    market.deposit_the_name().await?;

    let buyer = funded(&market.fleet, "walkin", 20).await?;
    mint_ft(&market.ft, &buyer, PRICE * 2).await?;
    let purse = Signing::new(buyer.clone());
    purse.register(&market.verifier).await?;
    assert_eq!(
        market.cash_of(&buyer).await?,
        0,
        "the buyer holds nothing inside the verifier before they act"
    );

    let ask = market.sell_side(PRICE as i128).await;
    let bid = purse.sign(
        &market.verifier,
        market.nonce().await,
        json!({
            "signer_id": buyer.id(),
            "deadline": market.far_future(),
            "intents": [{ "intent": "token_diff", "diff": {
                market.name_token.clone(): "1",
                market.cash_token.clone(): format!("-{PRICE}"),
            }}],
        }),
    );

    assert!(
        market
            .buy_in_one_call(&buyer, PRICE, vec![ask, bid])
            .await?,
        "one call must deposit the buyer's own tokens and settle the pair"
    );
    assert_eq!(
        held_amount(&market.verifier, &buyer, &market.name_token).await?,
        1,
        "the buyer holds the name"
    );
    assert_eq!(
        market.cash_of(&market.fleet.bob).await?,
        PRICE,
        "and the seller is paid, all in one receipt"
    );
    Ok(())
}

#[tokio::test]
async fn replaying_a_one_call_purchase_refunds_the_buyer() -> Result<()> {
    let market = open_market(0).await?;
    market.deposit_the_name().await?;

    let buyer = market.buyer_account.clone();
    mint_ft(&market.ft, &buyer, PRICE * 2).await?;

    let pair = vec![
        market.sell_side(PRICE as i128).await,
        market.buy_side(1).await,
    ];

    assert!(
        market.buy_in_one_call(&buyer, PRICE, pair.clone()).await?,
        "the first purchase lands"
    );
    let after_first = market.wallet_ft(&buyer).await?;

    assert!(
        !market.buy_in_one_call(&buyer, PRICE, pair).await?,
        "replaying the same pair must not settle a second time"
    );
    assert_eq!(
        market.wallet_ft(&buyer).await?,
        after_first,
        "and the buyer's tokens come back rather than being taken twice"
    );
    assert_eq!(
        market.name_held_by(&market.buyer_account).await?,
        1,
        "exactly one name moved"
    );
    Ok(())
}
