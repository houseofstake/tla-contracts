mod common;

use anyhow::Result;
use common::*;
use serde_json::json;

const MAX_MIGRATE_BATCH: usize = 25;

#[tokio::test]
async fn anyone_can_migrate_a_full_batch_in_one_call() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let tenant = rent(&fleet, &registry, &tla, "alice").await?;
    let before: serde_json::Value = fleet.worker.view(&tenant, "hos_lease").await?.json()?;

    let outcome = fleet
        .relay
        .call(fleet.extension.id(), "migrate_wallets")
        .args_json(json!({ "wallets": vec![tenant.clone(); MAX_MIGRATE_BATCH] }))
        .max_gas()
        .transact()
        .await?;
    assert!(outcome.is_success(), "{outcome:#?}");
    assert!(
        outcome.receipt_failures().is_empty(),
        "a full batch has to fund every migration it schedules: {:#?}",
        outcome.receipt_failures()
    );

    let after: serde_json::Value = fleet.worker.view(&tenant, "hos_lease").await?.json()?;
    assert_eq!(
        before, after,
        "migrating onto the same layout changes nothing"
    );
    Ok(())
}

#[tokio::test]
async fn a_batch_one_call_cannot_fund_is_refused() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let tenant = rent(&fleet, &registry, &tla, "alice").await?;

    let outcome = fleet
        .relay
        .call(fleet.extension.id(), "migrate_wallets")
        .args_json(json!({ "wallets": vec![tenant; MAX_MIGRATE_BATCH + 1] }))
        .max_gas()
        .transact()
        .await?;
    let err = format!("{:?}", outcome.into_result().unwrap_err());
    assert!(err.contains("batch_too_large"), "{err}");
    Ok(())
}
