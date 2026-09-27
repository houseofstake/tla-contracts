mod common;

use anyhow::{bail, Result};
use common::*;
use near_workspaces::result::{ExecutionFinalResult, ExecutionSuccess};
use near_workspaces::types::NearToken;
use near_workspaces::{Account, AccountId, Contract};
use serde_json::{json, Value};

const NAME: &str = "staff";

struct Business {
    fleet: Fleet,
    registry: Contract,
    tla: AccountId,
    tenant: AccountId,
    employee: Account,
}

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

async fn call(
    signer: &Account,
    registry: &Contract,
    method: &str,
    args: Value,
) -> Result<ExecutionFinalResult> {
    Ok(signer
        .call(registry.id(), method)
        .args_json(args)
        .deposit(NearToken::from_yoctonear(1))
        .max_gas()
        .transact()
        .await?)
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

async fn make_business(fleet: &Fleet, registry: &Contract, tla: &AccountId) -> Result<()> {
    landed(
        call(
            &fleet.council,
            registry,
            "admin_set_tla_type",
            json!({ "tla_id": tla, "tla_type": "Business", "licensee": fleet.bob.id() }),
        )
        .await?,
        "admin_set_tla_type",
    )?;
    Ok(())
}

async fn business_tla() -> Result<Business> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    make_business(&fleet, &registry, &tla).await?;
    landed(
        call(
            &fleet.council,
            &registry,
            "add_payment_authority",
            json!({ "account_id": fleet.relay.id() }),
        )
        .await?,
        "add_payment_authority",
    )?;
    landed(
        call(
            &fleet.council,
            &registry,
            "bind_payment_authority_tla",
            json!({ "account_id": fleet.relay.id(), "tla_id": tla, "max_mints": "1" }),
        )
        .await?,
        "bind_payment_authority_tla",
    )?;
    let employee = person(&fleet, "employee").await?;
    let minted = fleet
        .relay
        .call(registry.id(), "rent_sub_account_paid")
        .args_json(json!({
            "tla_id": tla,
            "name": NAME,
            "owner_account": employee.id(),
            "payout_account": fleet.bob.id(),
            "order_id": "ord-staff",
        }))
        .deposit(NearToken::from_millinear(300))
        .max_gas()
        .transact()
        .await?;
    landed(minted, "paid mint")?;
    let tenant: AccountId = format!("{NAME}.{tla}").parse()?;
    settled_wallet(&fleet.worker, &tenant).await?;
    let business = Business {
        fleet,
        registry,
        tla,
        tenant,
        employee,
    };
    assert_eq!(
        business.sub().await?["owner"],
        business.employee.id().as_str()
    );
    assert_eq!(
        business.sub().await?["payout_account"],
        business.fleet.bob.id().as_str(),
        "a paid business mint pays out to the licensee"
    );
    Ok(business)
}

impl Business {
    async fn sub(&self) -> Result<Value> {
        Ok(self
            .registry
            .view("get_sub_account")
            .args_json(json!({ "tla_id": self.tla, "name": NAME }))
            .await?
            .json()?)
    }

    async fn lease_until(&self) -> Result<String> {
        let lease: Value = self
            .fleet
            .worker
            .view(&self.tenant, "hos_lease")
            .await?
            .json()?;
        Ok(lease["lease_until_ns"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }

    async fn wallet_payout(&self) -> Result<String> {
        Ok(self
            .fleet
            .worker
            .view(&self.tenant, "hos_payout_account")
            .await?
            .json()?)
    }

    async fn wallet_owner(&self) -> Result<String> {
        owner_account(&self.fleet.worker, &self.tenant, self.fleet.extension.id()).await
    }

    async fn retraction_at(&self) -> Result<Option<u64>> {
        Ok(self
            .registry
            .view("get_retraction_at")
            .args_json(json!({ "tla_id": self.tla, "name": NAME }))
            .await?
            .json()?)
    }

    async fn open_resale(&self) -> Result<()> {
        landed(
            call(
                &self.fleet.council,
                &self.registry,
                "enable_business_resale",
                json!({ "tla_id": self.tla }),
            )
            .await?,
            "enable_business_resale",
        )?;
        Ok(())
    }

    async fn name_call(&self, signer: &Account, method: &str) -> Result<ExecutionFinalResult> {
        call(
            signer,
            &self.registry,
            method,
            json!({ "tla_id": self.tla, "name": NAME }),
        )
        .await
    }

    async fn sell(&self, seller: &Account, buyer: &AccountId) -> Result<ExecutionFinalResult> {
        call(
            seller,
            &self.registry,
            "nft_transfer",
            json!({
                "receiver_id": buyer,
                "token_id": self.tenant,
                "approval_id": null,
                "memo": null,
            }),
        )
        .await
    }

    async fn set_payout(&self, signer: &Account, to: &AccountId) -> Result<ExecutionFinalResult> {
        call(
            signer,
            &self.registry,
            "set_payout_account",
            json!({ "tla_id": self.tla, "name": NAME, "new_payout_account": to }),
        )
        .await
    }
}

#[tokio::test]
async fn only_hos_opens_resale_and_only_on_a_business_tla() -> Result<()> {
    let fleet = deploy_fleet().await?;
    let registry = deploy_registry(&fleet).await?;
    let tla = fleet.registrar.id().clone();
    let enable = json!({ "tla_id": tla });

    let on_open = call(
        &fleet.council,
        &registry,
        "enable_business_resale",
        enable.clone(),
    )
    .await?;
    assert!(
        refused_with(&on_open, "not_business_tla"),
        "an open TLA has nothing to open: {:?}",
        on_open.failures()
    );

    make_business(&fleet, &registry, &tla).await?;
    for outsider in [&fleet.bob, &fleet.relay] {
        let tried = call(
            outsider,
            &registry,
            "enable_business_resale",
            enable.clone(),
        )
        .await?;
        assert!(
            refused_with(&tried, "only_admin_or_council"),
            "{} opened resale: {:?}",
            outsider.id(),
            tried.failures()
        );
    }
    let unpaid = fleet
        .council
        .call(registry.id(), "enable_business_resale")
        .args_json(enable.clone())
        .max_gas()
        .transact()
        .await?;
    assert!(refused_with(&unpaid, "requires_one_yocto"));
    let closed: bool = registry
        .view("is_business_resale_enabled")
        .args_json(enable.clone())
        .await?
        .json()?;
    assert!(!closed, "a refused call must leave resale closed");

    let operator = person(&fleet, "operator").await?;
    landed(
        call(
            &fleet.council,
            &registry,
            "add_admin",
            json!({ "account_id": operator.id() }),
        )
        .await?,
        "add_admin",
    )?;
    let opened = landed(
        call(
            &operator,
            &registry,
            "enable_business_resale",
            enable.clone(),
        )
        .await?,
        "operator enable_business_resale",
    )?;
    assert!(
        opened
            .logs()
            .iter()
            .any(|log| log.contains("business_resale_enabled")),
        "opening resale must announce itself: {:?}",
        opened.logs()
    );
    let open: bool = registry
        .view("is_business_resale_enabled")
        .args_json(enable.clone())
        .await?
        .json()?;
    assert!(open);

    let again = landed(
        call(&fleet.council, &registry, "enable_business_resale", enable).await?,
        "council enable_business_resale",
    )?;
    assert!(
        !again
            .logs()
            .iter()
            .any(|log| log.contains("business_resale_enabled")),
        "a second open is a no-op and must not announce again"
    );
    Ok(())
}

#[tokio::test]
async fn until_resale_opens_the_licensee_governs_a_business_name() -> Result<()> {
    let business = business_tla().await?;
    let fleet = &business.fleet;
    let buyer = person(fleet, "buyer").await?;

    let sold = business.sell(&business.employee, buyer.id()).await?;
    assert!(refused_with(&sold, "business_sub_not_resellable"));
    let moved = call(
        &business.employee,
        &business.registry,
        "transfer_sub_account",
        json!({ "tla_id": business.tla, "name": NAME, "new_owner": buyer.id() }),
    )
    .await?;
    assert!(refused_with(&moved, "business_sub_not_resellable"));
    let deposited = call(
        &business.employee,
        &business.registry,
        "nft_transfer_call",
        json!({
            "receiver_id": buyer.id(),
            "token_id": business.tenant,
            "approval_id": null,
            "memo": null,
            "msg": "",
        }),
    )
    .await?;
    assert!(refused_with(&deposited, "business_sub_not_resellable"));
    assert_eq!(
        business.sub().await?["owner"],
        business.employee.id().as_str()
    );
    assert_eq!(
        business.wallet_owner().await?,
        business.employee.id().as_str()
    );

    let by_owner = business
        .set_payout(&business.employee, business.employee.id())
        .await?;
    assert!(refused_with(&by_owner, "only_licensee"));
    landed(
        business.set_payout(&fleet.bob, fleet.relay.id()).await?,
        "licensee set_payout_account",
    )?;
    assert_eq!(
        business.sub().await?["payout_account"],
        fleet.relay.id().as_str()
    );
    assert_eq!(business.wallet_payout().await?, fleet.relay.id().as_str());

    let by_hos = business
        .name_call(&fleet.council, "schedule_retraction")
        .await?;
    assert!(
        refused_with(&by_hos, "only_licensee"),
        "with resale closed, retraction stays with the licensee: {:?}",
        by_hos.failures()
    );
    let expires_at = business.sub().await?["expires_at"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    landed(
        business
            .name_call(&fleet.bob, "schedule_retraction")
            .await?,
        "licensee schedule_retraction",
    )?;
    assert!(business.retraction_at().await?.is_some());
    assert!(
        business.lease_until().await?.parse::<u64>()? < expires_at.parse::<u64>()?,
        "the wallet lease must be cut to the notice period"
    );
    landed(
        business.name_call(&fleet.bob, "cancel_retraction").await?,
        "licensee cancel_retraction",
    )?;
    assert!(business.retraction_at().await?.is_none());
    assert_eq!(business.lease_until().await?, expires_at);
    Ok(())
}

#[tokio::test]
async fn once_resale_opens_the_owner_sells_and_the_buyer_holds_the_name() -> Result<()> {
    let business = business_tla().await?;
    business.open_resale().await?;
    let buyer = person(&business.fleet, "buyer").await?;

    landed(
        business.sell(&business.employee, buyer.id()).await?,
        "sale to the buyer",
    )?;
    let sub = business.sub().await?;
    assert_eq!(sub["owner"], buyer.id().as_str());
    assert_eq!(
        sub["payout_account"],
        buyer.id().as_str(),
        "the licensee must not keep the payout of a name it no longer owns"
    );
    assert_eq!(business.wallet_payout().await?, buyer.id().as_str());
    assert_eq!(business.wallet_owner().await?, buyer.id().as_str());
    let token: Value = business
        .registry
        .view("nft_token")
        .args_json(json!({ "token_id": business.tenant }))
        .await?
        .json()?;
    assert_eq!(token["owner_id"], buyer.id().as_str());

    let next = person(&business.fleet, "nextbuyer").await?;
    landed(
        call(
            &buyer,
            &business.registry,
            "transfer_sub_account",
            json!({ "tla_id": business.tla, "name": NAME, "new_owner": next.id() }),
        )
        .await?,
        "resale by the buyer",
    )?;
    assert_eq!(business.sub().await?["owner"], next.id().as_str());
    assert_eq!(business.wallet_owner().await?, next.id().as_str());
    Ok(())
}

#[tokio::test]
async fn once_resale_opens_payout_is_the_owners_and_only_hos_retracts() -> Result<()> {
    let business = business_tla().await?;
    business.open_resale().await?;
    let fleet = &business.fleet;

    let by_licensee = business.set_payout(&fleet.bob, fleet.bob.id()).await?;
    assert!(refused_with(&by_licensee, "only_owner"));
    landed(
        business
            .set_payout(&business.employee, business.employee.id())
            .await?,
        "owner set_payout_account",
    )?;
    assert_eq!(
        business.sub().await?["payout_account"],
        business.employee.id().as_str()
    );
    assert_eq!(
        business.wallet_payout().await?,
        business.employee.id().as_str()
    );

    let by_licensee = business
        .name_call(&fleet.bob, "schedule_retraction")
        .await?;
    assert!(refused_with(&by_licensee, "only_admin_or_council"));
    let expires_at = business.sub().await?["expires_at"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    landed(
        business
            .name_call(&fleet.council, "schedule_retraction")
            .await?,
        "hos schedule_retraction",
    )?;
    assert!(business.retraction_at().await?.is_some());
    assert!(
        business.lease_until().await?.parse::<u64>()? < expires_at.parse::<u64>()?,
        "the wallet lease must be cut to the notice period"
    );

    let buyer = person(fleet, "buyer").await?;
    let during = business.sell(&business.employee, buyer.id()).await?;
    assert!(
        refused_with(&during, "retraction_pending"),
        "a name under retraction must not change hands: {:?}",
        during.failures()
    );

    let by_licensee = business.name_call(&fleet.bob, "cancel_retraction").await?;
    assert!(refused_with(&by_licensee, "only_admin_or_council"));
    landed(
        business
            .name_call(&fleet.council, "cancel_retraction")
            .await?,
        "hos cancel_retraction",
    )?;
    assert!(business.retraction_at().await?.is_none());
    assert_eq!(business.lease_until().await?, expires_at);

    landed(
        business.sell(&business.employee, buyer.id()).await?,
        "sale after the retraction is lifted",
    )?;
    assert_eq!(business.sub().await?["owner"], buyer.id().as_str());
    Ok(())
}
