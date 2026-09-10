mod common;

use anyhow::Result;
use common::{deploy_fleet, deploy_registry};
use near_sdk::NearToken;
use near_workspaces::Contract;
use serde_json::json;

fn seed_names(prefix: &str, count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{prefix}{index}")).collect()
}

async fn try_batch(
    fleet: &common::Fleet,
    registry: &Contract,
    prefix: &str,
    count: usize,
) -> Result<Option<(u64, u64)>> {
    let names = seed_names(prefix, count);
    let registered = fleet
        .council
        .call(registry.id(), "register_tlas")
        .args_json(json!({
            "tla_ids": names,
            "tla_type": "Open",
            "premium_category": "Standard",
        }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    let Ok(registered) = registered.into_result() else {
        return Ok(None);
    };
    let opened = fleet
        .council
        .call(registry.id(), "activate_open_tlas")
        .args_json(json!({ "tla_ids": names }))
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?;
    let Ok(opened) = opened.into_result() else {
        return Ok(None);
    };
    Ok(Some((
        registered.total_gas_burnt.as_gas() / 1_000_000_000_000,
        opened.total_gas_burnt.as_gas() / 1_000_000_000_000,
    )))
}

#[tokio::test]
async fn a_full_batch_at_the_contract_cap_executes_in_one_transaction() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let cap: usize = registry.view("max_tla_batch").await?.json()?;

    let measured = try_batch(&fleet, &registry, "longestseedname", cap).await?;
    let Some((register_tgas, open_tgas)) = measured else {
        panic!("the contract accepts {cap} names but the runtime cannot execute that many");
    };
    println!("{cap} names: register {register_tgas} Tgas, open {open_tgas} Tgas");
    println!("calls needed for 3900 names: {}", 3_900usize.div_ceil(cap));
    assert!(
        register_tgas < 270,
        "a full batch burned {register_tgas} Tgas of the 300 available, which leaves \
         no room for a longer name than this test used"
    );
    Ok(())
}
