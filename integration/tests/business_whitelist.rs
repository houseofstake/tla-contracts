mod common;

use anyhow::{bail, Result};
use common::*;
use near_workspaces::result::{ExecutionFinalResult, ExecutionSuccess};
use near_workspaces::types::NearToken;
use near_workspaces::{Account, AccountId, Contract};
use serde_json::{json, Value};

const SHORT_TERM_NS: u64 = 60 * 1_000_000_000;
const BLOCKS_PAST_TERM_AND_GRACE: u64 = 4_000;

async fn person(fleet: &Fleet, label: &str) -> Result<Account> {
    Ok(fleet
        .worker
        .root_account()?
        .create_subaccount(label)
        .initial_balance(NearToken::from_near(10))
        .transact()
        .await?
        .into_result()?)
}

fn landed(outcome: ExecutionFinalResult, what: &str) -> Result<ExecutionSuccess> {
    let outcome = outcome.into_result()?;
    if let Some(failure) = outcome.receipt_failures().first() {
        bail!("{what} left a failed receipt: {failure:?}");
    }
    Ok(outcome)
}

fn refused_with(outcome: &ExecutionFinalResult, code: &str) -> bool {
    outcome.is_failure() && format!("{:?}", outcome.failures()).contains(code)
}

async fn send(
    signer: &Account,
    registry: &Contract,
    method: &str,
    args: Value,
    deposit: NearToken,
) -> Result<ExecutionFinalResult> {
    Ok(signer
        .call(registry.id(), method)
        .args_json(args)
        .deposit(deposit)
        .max_gas()
        .transact()
        .await?)
}

async fn claim(
    member: &Account,
    registry: &Contract,
    tla: &AccountId,
    name: &str,
) -> Result<ExecutionFinalResult> {
    let config: Value = registry.view("get_fee_config").await?.json()?;
    let deposit: u128 = config["account_creation_deposit_yocto"]
        .as_str()
        .unwrap_or_default()
        .parse()?;
    send(
        member,
        registry,
        "claim_business_name",
        json!({ "tla_id": tla, "name": name }),
        NearToken::from_yoctonear(deposit),
    )
    .await
}

#[tokio::test]
async fn an_operator_whitelists_a_member_who_claims_one_unsellable_name() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let one_yocto = NearToken::from_yoctonear(1);
    landed(
        send(
            &fleet.council,
            &registry,
            "admin_set_tla_type",
            json!({ "tla_id": tla, "tla_type": "Business", "licensee": fleet.bob.id() }),
            one_yocto,
        )
        .await?,
        "admin_set_tla_type",
    )?;
    let operator = person(&fleet, "operator").await?;
    landed(
        send(
            &fleet.council,
            &registry,
            "add_admin",
            json!({ "account_id": operator.id() }),
            one_yocto,
        )
        .await?,
        "add_admin",
    )?;
    let member = person(&fleet, "member").await?;
    let stranger = person(&fleet, "stranger").await?;

    let by_stranger = send(
        &stranger,
        &registry,
        "admin_whitelist_add",
        json!({ "tla_id": tla, "accounts": [stranger.id()] }),
        one_yocto,
    )
    .await?;
    assert!(refused_with(&by_stranger, "only_admin"));
    landed(
        send(
            &operator,
            &registry,
            "admin_whitelist_add",
            json!({ "tla_id": tla, "accounts": [member.id()] }),
            one_yocto,
        )
        .await?,
        "admin_whitelist_add",
    )?;
    let listed: bool = registry
        .view("is_whitelisted")
        .args_json(json!({ "tla_id": tla, "account_id": member.id() }))
        .await?
        .json()?;
    assert!(listed);

    assert!(refused_with(
        &claim(&stranger, &registry, &tla, "stranger").await?,
        "not_whitelisted"
    ));
    landed(claim(&member, &registry, &tla, "member").await?, "claim")?;
    let full: AccountId = format!("member.{tla}").parse()?;
    settled_wallet(&fleet.worker, &full).await?;
    let sub: Value = registry
        .view("get_sub_account")
        .args_json(json!({ "tla_id": tla, "name": "member" }))
        .await?
        .json()?;
    assert_eq!(sub["owner"], member.id().as_str());
    assert_eq!(
        sub["payout_account"],
        member.id().as_str(),
        "a claimed business name pays out to the member who holds it"
    );
    let wallet_payout: String = fleet
        .worker
        .view(&full, "hos_payout_account")
        .await?
        .json()?;
    assert_eq!(
        wallet_payout,
        member.id().as_str(),
        "the wallet sweeps to the member, not the licensee"
    );

    assert!(refused_with(
        &claim(&member, &registry, &tla, "member2").await?,
        "not_whitelisted"
    ));
    let moved = send(
        &member,
        &registry,
        "transfer_sub_account",
        json!({ "tla_id": tla, "name": "member", "new_owner": stranger.id() }),
        one_yocto,
    )
    .await?;
    assert!(refused_with(&moved, "business_sub_not_resellable"));
    Ok(())
}

#[tokio::test]
async fn a_claimed_wallet_pays_out_where_its_member_says_and_never_to_the_licensee() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry_with_terms(&fleet, Some(SHORT_TERM_NS), SHORT_TERM_NS).await?;
    let tla = fleet.registrar.id().clone();
    let one_yocto = NearToken::from_yoctonear(1);
    landed(
        send(
            &fleet.council,
            &registry,
            "admin_set_tla_type",
            json!({ "tla_id": tla, "tla_type": "Business", "licensee": fleet.bob.id() }),
            one_yocto,
        )
        .await?,
        "admin_set_tla_type",
    )?;
    let member = person(&fleet, "member").await?;
    let vault = person(&fleet, "vault").await?;
    landed(
        send(
            &fleet.council,
            &registry,
            "admin_whitelist_add",
            json!({ "tla_id": tla, "accounts": [member.id()] }),
            one_yocto,
        )
        .await?,
        "admin_whitelist_add",
    )?;
    landed(claim(&member, &registry, &tla, "keeper").await?, "claim")?;
    let wallet: AccountId = format!("keeper.{tla}").parse()?;
    settled_wallet(&fleet.worker, &wallet).await?;

    let by_licensee = send(
        &fleet.bob,
        &registry,
        "set_payout_account",
        json!({ "tla_id": tla, "name": "keeper", "new_payout_account": fleet.bob.id() }),
        one_yocto,
    )
    .await?;
    assert!(refused_with(&by_licensee, "only_owner"));
    landed(
        send(
            &member,
            &registry,
            "set_payout_account",
            json!({ "tla_id": tla, "name": "keeper", "new_payout_account": vault.id() }),
            one_yocto,
        )
        .await?,
        "set_payout_account",
    )?;
    let payout: String = fleet
        .worker
        .view(&wallet, "hos_payout_account")
        .await?
        .json()?;
    assert_eq!(payout, vault.id().as_str());

    fleet
        .relay
        .transfer_near(&wallet, NearToken::from_near(5))
        .await?
        .into_result()?;
    fleet
        .worker
        .fast_forward(BLOCKS_PAST_TERM_AND_GRACE)
        .await?;
    let licensee_before = balance_of(&fleet.worker, fleet.bob.id()).await?;
    let vault_before = balance_of(&fleet.worker, vault.id()).await?;
    landed(
        fleet
            .relay
            .call(registry.id(), "reclaim_sweep_near")
            .args_json(json!({ "tla_id": tla, "name": "keeper" }))
            .deposit(one_yocto)
            .max_gas()
            .transact()
            .await?,
        "reclaim_sweep_near",
    )?;
    assert!(
        balance_of(&fleet.worker, vault.id()).await? > vault_before,
        "the lapsed wallet's balance must reach the account its member chose"
    );
    assert_eq!(
        balance_of(&fleet.worker, fleet.bob.id()).await?,
        licensee_before,
        "the licensee is paid nothing out of a member's wallet"
    );
    Ok(())
}
