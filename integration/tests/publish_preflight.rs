use anyhow::{bail, Context, Result};
use hos_wallet::LegacyTenantWallet;
use near_sdk::borsh::BorshDeserialize;
use near_workspaces::types::AccountId;
use serde_json::json;
use sha2::{Digest, Sha256};

const DEFAULT_RPC: &str = "https://test.rpc.fastnear.com";
const REGISTRY: &str = "registry.hosdemo.testnet";
const COUNCIL: &str = "council.hosdemo.testnet";
const WALLET_WASM: &str = "../target/near/hos_wallet/hos_wallet.wasm";
const LAYOUT_SAMPLE: usize = 5;
const YOCTO_PER_GLOBAL_BYTE: u128 = 100_000_000_000_000_000_000;
const ONE_NEAR: u128 = 1_000_000_000_000_000_000_000_000;
const GAS_AND_RESERVE: u128 = 2 * ONE_NEAR;

type Testnet = near_workspaces::Worker<near_workspaces::network::Testnet>;

fn rpc_url() -> String {
    std::env::var("NEAR_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC.to_string())
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

fn agree<T: PartialEq + std::fmt::Debug>(
    account: &str,
    field: &str,
    from_bytes: T,
    from_view: T,
) -> Result<()> {
    if from_bytes != from_view {
        bail!(
            "{account}: decoding deployed state with LegacyTenantWallet reads {field} \
             as {from_bytes:?}, but the contract reports {from_view:?}. The migration \
             is built on a layout that is not the one deployed, so publishing would \
             leave every leased account unreadable."
        );
    }
    Ok(())
}

fn text(view: &serde_json::Value, key: &str, account: &str) -> Result<String> {
    Ok(view
        .get(key)
        .and_then(|v| v.as_str())
        .with_context(|| format!("{account}: view is missing {key}"))?
        .to_string())
}

async fn layout_holds_for(worker: &Testnet, name: &str) -> Result<()> {
    let id: AccountId = name.parse()?;

    let raw = worker
        .view_state(&id)
        .await?
        .remove(Vec::new().as_slice())
        .with_context(|| format!("{name} holds no contract state"))?;

    let decoded = LegacyTenantWallet::try_from_slice(&raw).map_err(|err| {
        anyhow::anyhow!(
            "{name}: LegacyTenantWallet cannot decode deployed state ({err}). \
             Publishing would leave every leased account unreadable, with no \
             migration able to recover them."
        )
    })?;

    let lease: serde_json::Value = worker.view(&id, "hos_lease").await?.json()?;
    let item: serde_json::Value = worker.view(&id, "nft_item_info").await?.json()?;
    let reported_extensions: Vec<String> = worker.view(&id, "w_extensions").await?.json()?;

    agree(
        name,
        "authority",
        decoded.authority.to_string(),
        text(&lease, "authority", name)?,
    )?;
    agree(
        name,
        "payout_account",
        decoded.payout_account.to_string(),
        text(&lease, "payout_account", name)?,
    )?;
    agree(
        name,
        "lease_until_ns",
        decoded.lease_until_ns.to_string(),
        text(&lease, "lease_until_ns", name)?,
    )?;
    agree(
        name,
        "state",
        serde_json::to_value(decoded.state)?,
        lease
            .get("state")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )?;
    agree(
        name,
        "owner",
        decoded.owner.to_string(),
        text(&item, "owner_id", name)?,
    )?;
    agree(
        name,
        "collection_id",
        decoded.collection_id.to_string(),
        text(&item, "collection_id", name)?,
    )?;

    let mut from_bytes: Vec<String> = decoded
        .wallet
        .extensions
        .iter()
        .map(|a| a.to_string())
        .collect();
    let mut from_view = reported_extensions;
    from_bytes.sort();
    from_view.sort();
    agree(name, "extensions", from_bytes, from_view)?;

    Ok(())
}

async fn the_publish_is_affordable(worker: &Testnet) -> Result<()> {
    let wasm = std::fs::read(WALLET_WASM).with_context(|| {
        format!(
            "{WALLET_WASM} not found. Build it first with \
             `cargo near build non-reproducible-wasm --locked --no-abi`"
        )
    })?;
    let digest = format!("{:x}", Sha256::digest(&wasm));
    println!("wallet wasm: {} bytes, sha256 {digest}", wasm.len());

    match std::env::var("EXPECT_WASM_SHA256") {
        Ok(expected) if expected.eq_ignore_ascii_case(&digest) => {
            println!("wasm matches the artifact this publish is authorised for");
        }
        Ok(expected) => bail!(
            "the built wasm hashes to {digest} but EXPECT_WASM_SHA256 is {expected}. \
             Approving one artifact and publishing another is how a fleet ends up \
             running code nothing verified."
        ),
        Err(_) => bail!(
            "set EXPECT_WASM_SHA256={digest} to bind this run to the artifact you are \
             about to approve. Without it nothing ties what was checked to what gets \
             published."
        ),
    }

    let payer_id = std::env::var("PUBLISH_PAYER").unwrap_or_else(|_| COUNCIL.to_string());
    let payer: AccountId = payer_id.parse()?;
    let balance = worker.view_account(&payer).await?.balance.as_yoctonear();
    let attached = wasm.len() as u128 * YOCTO_PER_GLOBAL_BYTE;
    let needed = attached + GAS_AND_RESERVE;
    println!(
        "{payer_id} holds {} NEAR; gd_deploy attaches {} NEAR",
        balance / ONE_NEAR,
        attached / ONE_NEAR
    );
    if balance < needed {
        let short = (needed - balance) / ONE_NEAR + 1;
        bail!(
            "{payer_id} cannot fund this publish. gd_deploy takes the cost as an \
             attached deposit from whoever calls it, so the caller needs {} NEAR \
             plus a reserve and holds {}. Fund it with about {short} more NEAR. \
             A gd_approve without the deposit to follow it strands the approval.",
            attached / ONE_NEAR,
            balance / ONE_NEAR
        );
    }
    Ok(())
}

#[tokio::test]
async fn the_fleet_is_safe_to_republish() -> Result<()> {
    let worker = near_workspaces::testnet().rpc_addr(&rpc_url()).await?;

    let names = leased_names(&worker).await?;
    println!("leased names: {}", names.len());
    if names.is_empty() {
        bail!("the collection reports no names, which is never true of a live fleet");
    }

    let mut versions = Vec::new();
    let mut unreadable = Vec::new();
    for name in &names {
        let id: AccountId = name.parse()?;
        match worker.view(&id, "hos_lease").await {
            Ok(view) => {
                let lease: serde_json::Value = view.json()?;
                versions.push(
                    lease
                        .get("impl_version")
                        .and_then(|v| v.as_u64())
                        .context("hos_lease without impl_version")?,
                );
            }
            Err(_) => unreadable.push(name.clone()),
        }
    }

    if !unreadable.is_empty() {
        for name in unreadable.iter().take(10) {
            println!("  unreadable: {name}");
        }
        bail!(
            "{} leased accounts cannot be read. Publishing replaces the code under \
             accounts whose state nothing can currently migrate.",
            unreadable.len()
        );
    }

    if let Ok(deployer) = std::env::var("DEPLOYER_ACCOUNT") {
        let id: AccountId = deployer
            .parse()
            .context("DEPLOYER_ACCOUNT must be an account")?;
        let cfg: serde_json::Value = worker.view(&id, "config").await?.json()?;
        let delay = cfg
            .get("approval_delay_ns")
            .and_then(|v| v.as_str())
            .context("deployer config has no approval_delay_ns")?;
        let production = cfg
            .get("production_delay")
            .and_then(serde_json::Value::as_bool)
            .context("deployer config has no production_delay")?;
        println!("deployer approval_delay_ns: {delay} (production: {production})");
        if std::env::var("REQUIRE_PRODUCTION_DELAY").is_ok() && !production {
            bail!(
                "the deployer publishes with approval_delay_ns={delay}. A publish replaces the \
                 code under every leased account, and the delay is the only window in which an \
                 owner can see an approved hash before that happens."
            );
        }
    }

    let mut distinct = versions.clone();
    distinct.sort_unstable();
    distinct.dedup();
    println!("impl versions present: {distinct:?}");
    if distinct.len() != 1 {
        bail!(
            "the fleet spans {} impl versions. One legacy struct describes one \
             layout, so a single migration cannot serve them all.",
            distinct.len()
        );
    }
    let live_version = distinct[0];

    if let Ok(expected) = std::env::var("EXPECT_IMPL_VERSION") {
        let want: u64 = expected
            .parse()
            .context("EXPECT_IMPL_VERSION must be a number")?;
        if live_version != want {
            bail!("fleet is on impl_version {live_version}, expected {want}");
        }
        println!("all {} accounts are on impl_version {want}", names.len());
        return Ok(());
    }

    let step = std::cmp::max(1, names.len() / LAYOUT_SAMPLE);
    let sampled: Vec<&String> = names.iter().step_by(step).take(LAYOUT_SAMPLE).collect();
    println!("checking the deployed layout on {} accounts", sampled.len());
    for name in &sampled {
        layout_holds_for(&worker, name).await?;
        println!("  layout holds: {name}");
    }

    the_publish_is_affordable(&worker).await?;

    println!("\nfleet uniform on impl_version {live_version}, safe to publish");
    Ok(())
}
