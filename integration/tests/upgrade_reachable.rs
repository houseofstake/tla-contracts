mod common;

use anyhow::{bail, Result};
use common::*;
use near_sdk::json_types::{Base64VecU8, U128};
use near_workspaces::types::NearToken;
use near_workspaces::{Account, AccountId};
use serde_json::json;
use sha2::{Digest, Sha256};

const YOCTO: NearToken = NearToken::from_yoctonear(1);

struct Entry {
    contract: AccountId,
    method: &'static str,
    args: serde_json::Value,
}

async fn refusal(caller: &Account, entry: &Entry, deposit: NearToken) -> Result<String> {
    let outcome = caller
        .call(&entry.contract, entry.method)
        .args_json(entry.args.clone())
        .deposit(deposit)
        .max_gas()
        .transact()
        .await?;
    Ok(match outcome.into_result() {
        Ok(_) => String::new(),
        Err(err) => format!("{err:?}"),
    })
}

async fn assert_reachable(caller: &Account, entry: &Entry) -> Result<()> {
    let label = format!("{}.{}", entry.contract, entry.method);

    let with_yocto = refusal(caller, entry, YOCTO).await?;
    if with_yocto.contains("doesn't accept deposit") {
        bail!(
            "{label} is unreachable: it demands one yocto and the wrapper rejects any deposit, \
             so no caller can ever execute it. Add #[payable]."
        );
    }

    let without = refusal(caller, entry, NearToken::from_yoctonear(0)).await?;
    if !without.contains("requires_one_yocto") && !without.contains("exactly 1 yoctoNEAR") {
        bail!(
            "{label} accepted a call with no deposit, so a function-call access key scoped to it \
             can drive it. Privileged methods must assert one yocto. Got: {without}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn every_privileged_entry_point_is_reachable_and_key_gated() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;

    let code_hash = bs58::encode(Sha256::digest(wasm("registrar"))).into_string();
    let unapproved = json!({ "code": Base64VecU8(vec![0, 97, 115, 109]) });
    let stray_key = "ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847";

    let entries = vec![
        Entry {
            contract: registry.id().clone(),
            method: "approve_upgrade",
            args: json!({ "code_hash": code_hash }),
        },
        Entry {
            contract: registry.id().clone(),
            method: "upgrade",
            args: unapproved.clone(),
        },
        Entry {
            contract: fleet.registrar.id().clone(),
            method: "approve_upgrade",
            args: json!({ "code_hash": code_hash }),
        },
        Entry {
            contract: fleet.registrar.id().clone(),
            method: "upgrade_self",
            args: unapproved.clone(),
        },
        Entry {
            contract: fleet.extension.id().clone(),
            method: "approve_upgrade",
            args: json!({ "code_hash": code_hash }),
        },
        Entry {
            contract: fleet.extension.id().clone(),
            method: "upgrade",
            args: unapproved.clone(),
        },
        Entry {
            contract: fleet.recovery.id().clone(),
            method: "approve_upgrade",
            args: json!({ "code_hash": code_hash }),
        },
        Entry {
            contract: fleet.recovery.id().clone(),
            method: "upgrade",
            args: unapproved.clone(),
        },
        Entry {
            contract: fleet.deployer.id().clone(),
            method: "gd_approve",
            args: json!({ "hash": code_hash }),
        },
        Entry {
            contract: fleet.deployer.id().clone(),
            method: "approve_self_upgrade",
            args: json!({ "hash": code_hash }),
        },
        Entry {
            contract: fleet.deployer.id().clone(),
            method: "upgrade_self",
            args: unapproved.clone(),
        },
        Entry {
            contract: fleet.deployer.id().clone(),
            method: "gd_delete_key",
            args: json!({ "public_key": stray_key }),
        },
        Entry {
            contract: fleet.extension.id().clone(),
            method: "skim",
            args: json!({ "amount": U128(1) }),
        },
    ];

    for entry in &entries {
        assert_reachable(&fleet.council, entry).await?;
    }
    Ok(())
}

#[tokio::test]
async fn the_registrar_config_setters_are_reachable_and_key_gated() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let entries = vec![
        Entry {
            contract: fleet.registrar.id().clone(),
            method: "set_min_label_len",
            args: json!({ "min_label_len": 3 }),
        },
        Entry {
            contract: fleet.registrar.id().clone(),
            method: "set_min_balance",
            args: json!({ "min_balance": NearToken::from_millinear(100) }),
        },
    ];
    for entry in &entries {
        assert_reachable(&fleet.council, entry).await?;
    }
    Ok(())
}
