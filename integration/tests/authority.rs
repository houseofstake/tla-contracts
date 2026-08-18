mod common;

use anyhow::{bail, Result};
use common::*;
use near_workspaces::types::NearToken;
use serde_json::json;

#[tokio::test]
async fn a_compromised_registry_cannot_take_a_live_name() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let tenant = rent(&fleet, &registry, &tla, "alice").await?;

    for asked_by in [
        Some(fleet.extension.id().clone()),
        Some(fleet.council.id().clone()),
        Some(fleet.relay.id().clone()),
        None,
    ] {
        let attempt = fleet
            .registry
            .call(fleet.extension.id(), "force_transfer")
            .args_json(json!({
                "wallet": tenant,
                "new_owner": fleet.relay.id(),
                "cause": "Transfer",
                "asked_by": asked_by,
            }))
            .max_gas()
            .transact()
            .await?;
        let landed = attempt.is_success() && attempt.receipt_failures().is_empty();
        assert!(
            !landed,
            "a registry asking on its own behalf moved the name with asked_by {asked_by:?}"
        );
        assert_eq!(
            owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
            fleet.bob.id().as_str(),
            "the owner must not move for asked_by {asked_by:?}"
        );
    }

    Ok(())
}

#[tokio::test]
async fn recovery_moves_a_name_with_nothing_armed_beforehand() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let tenant = rent(&fleet, &registry, &tla, "alice").await?;

    fleet
        .council
        .call(registry.id(), "add_recovery_authority")
        .args_json(json!({ "account_id": fleet.recovery.id() }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;

    let recovered = fleet
        .recovery
        .call(registry.id(), "recover_sub_account")
        .args_json(json!({
            "tla_id": tla,
            "name": "alice",
            "new_owner": fleet.relay.id(),
            "expected_owner": fleet.bob.id(),
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?
        .into_result()?;
    if let Some(failure) = recovered.receipt_failures().first() {
        bail!("recovery receipt failed: {failure:?}");
    }

    assert_eq!(
        owner_account(&fleet.worker, &tenant, fleet.extension.id()).await?,
        fleet.relay.id().as_str(),
        "a user who lost access cannot arm anything, so recovery must stand alone"
    );

    Ok(())
}
