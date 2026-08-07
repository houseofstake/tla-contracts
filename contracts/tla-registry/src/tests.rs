use crate::error::ContractError;
use crate::types::*;
use crate::{fees, TlaRegistry};
use hos_common::MintOutcome;
use near_sdk::json_types::{U128, U64};
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::{testing_env, AccountId, NearToken, PromiseError};
use std::str::FromStr;

const ADMIN: &str = "hos.testnet";
const HOSEXT: &str = "hos-extension.testnet";
const TREASURY: &str = "treasury.testnet";
const COUNCIL: &str = ADMIN;
const OTHER_COUNCIL: &str = "council.testnet";
const TLA: &str = "mytla";
const ALICE: &str = "alice.testnet";
const BOB: &str = "bob.testnet";
const CAROL: &str = "carol.testnet";
const GRACE_NS: u64 = 30 * 24 * 60 * 60 * 1_000_000_000;
const DAY_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

fn acc(s: &str) -> AccountId {
    AccountId::from_str(s).unwrap()
}

fn ctx(predecessor: &str, deposit: u128, ts: u64) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(acc("registry.testnet"))
        .predecessor_account_id(acc(predecessor))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .block_timestamp(ts)
        .build());
}

fn ctx_callback(result: near_sdk::PromiseResult) {
    testing_env!(
        VMContextBuilder::new()
            .current_account_id(acc("registry.testnet"))
            .predecessor_account_id(acc("registry.testnet"))
            .build(),
        near_sdk::test_vm_config(),
        near_sdk::RuntimeFeesConfig::test(),
        Default::default(),
        vec![result],
    );
}

const NEAR_USD_MICRO: u128 = 5_000_000;

fn deploy() -> TlaRegistry {
    ctx(ADMIN, 0, 0);
    TlaRegistry::new(
        acc(ADMIN),
        acc(HOSEXT),
        U64(GRACE_NS),
        acc(TREASURY),
        acc(COUNCIL),
    )
}

fn deploy_priced() -> TlaRegistry {
    let mut c = deploy();
    ctx(ADMIN, 0, 0);
    c.admin_set_initial_rate(U128(NEAR_USD_MICRO)).unwrap();
    c
}

fn usd_to_near(usd_micro: u128) -> u128 {
    usd_micro * 1_000_000_000_000_000_000_000_000 / NEAR_USD_MICRO
}

fn deploy_with_open_tla() -> TlaRegistry {
    let mut c = deploy_priced();
    ctx(COUNCIL, 0, 0);
    c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
        .unwrap();
    ctx(ADMIN, 0, 0);
    c.activate_open_tla(acc(TLA)).unwrap();
    c
}

fn rent_total(c: &TlaRegistry, name: &str) -> u128 {
    let price = c.get_rent_price(acc(TLA), name.to_string()).unwrap();
    price.total_yocto.0
}

fn rent_usd_open(c: &TlaRegistry, name: &str) -> u128 {
    let total_len = (name.len() + 1 + TLA.len()) as u8;
    fees::sub_account_rent(total_len, &PremiumCategory::Standard, &c.get_fee_config())
}

fn rent_near_open(c: &TlaRegistry, name: &str) -> u128 {
    usd_to_near(rent_usd_open(c, name))
}

fn settled(
    name: &str,
    owner: &str,
    payer: &str,
    rent_yocto: u128,
    attached_yocto: u128,
) -> crate::callbacks::MintSettlement {
    crate::callbacks::MintSettlement {
        tla_id: acc(TLA),
        name: name.to_string(),
        owner: acc(owner),
        payer: acc(payer),
        rent_yocto: U128(rent_yocto),
        attached_yocto: U128(attached_yocto),
    }
}

fn rent_alice_sub(c: &mut TlaRegistry, name: &str) {
    let rent_near = rent_near_open(c, name);
    let deposit = c.get_fee_config().account_creation_deposit_yocto.0;
    let total = rent_near + deposit;
    ctx(ALICE, total, 1);
    let _ = c
        .rent_sub_account(acc(TLA), name.to_string(), None)
        .unwrap();
    ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
    c.on_sub_account_created(
        settled(name, ALICE, ALICE, rent_near, total),
        Ok(MintOutcome::Active),
    );
}

mod names {
    use super::*;

    #[test]
    fn valid_names_accepted() {
        assert!(validate_name("alice").is_ok());
        assert!(validate_name("a1-b_c").is_ok());
    }

    #[test]
    fn invalid_names_rejected() {
        assert!(validate_name("").is_err());
        assert!(validate_name(&"a".repeat(61)).is_err());
        assert!(validate_name("Alice").is_err());
        assert!(validate_name("has.dot").is_err());
        assert!(validate_name("-edge").is_err());
        assert!(validate_name("edge_").is_err());
    }
}

mod fee_math {
    use super::*;

    #[test]
    fn base_rent_tiers() {
        let config = fees::default_fee_config();
        assert_eq!(fees::base_rent(5, &config), config.rent_tier_5_usd_micro.0);
        assert_eq!(fees::base_rent(8, &config), config.rent_tier_8_usd_micro.0);
        assert_eq!(
            fees::base_rent(10, &config),
            config.rent_tier_10_usd_micro.0
        );
        assert_eq!(
            fees::base_rent(20, &config),
            config.rent_tier_12plus_usd_micro.0
        );
    }

    #[test]
    fn premium_multipliers_scale_rent() {
        let config = fees::default_fee_config();
        let standard = fees::sub_account_rent(20, &PremiumCategory::Standard, &config);
        let premium = fees::sub_account_rent(20, &PremiumCategory::Premium, &config);
        let legendary = fees::sub_account_rent(20, &PremiumCategory::Legendary, &config);
        let community = fees::sub_account_rent(20, &PremiumCategory::Community, &config);
        assert_eq!(standard, config.rent_tier_12plus_usd_micro.0 * 3 / 2);
        assert_eq!(premium, config.rent_tier_12plus_usd_micro.0 * 3);
        assert_eq!(legendary, config.rent_tier_12plus_usd_micro.0 * 5);
        assert_eq!(community, 0);
    }
}

mod tla_admin {
    use super::*;

    #[test]
    fn suspend_registered_tla_rejected() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
        assert!(matches!(
            c.suspend_tla(acc(TLA)),
            Err(ContractError::TlaNotActive)
        ));
    }

    #[test]
    fn register_and_activate_open_tla() {
        let c = deploy_with_open_tla();
        let view = c.get_tla(acc(TLA)).unwrap();
        assert!(matches!(view.lifecycle, LifecycleStatus::Active));
    }

    #[test]
    fn register_tla_emits_nep297_event() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
        let logs = near_sdk::test_utils::get_logs();
        let entry = logs
            .iter()
            .find(|l| l.starts_with("EVENT_JSON:"))
            .expect("registry emits an EVENT_JSON log");
        let json: near_sdk::serde_json::Value =
            near_sdk::serde_json::from_str(entry.trim_start_matches("EVENT_JSON:")).unwrap();
        assert_eq!(json["standard"], "hos_tla_registry");
        assert_eq!(json["version"], "1.0.0");
        assert_eq!(json["event"], "tla_registered");
        assert_eq!(json["data"]["tla_id"], TLA);
        assert_eq!(json["data"]["tla_type"], "Open");
        assert_eq!(json["data"]["premium_category"], "Standard");
        assert!(json["data"]["licensee"].is_null());
    }

    #[test]
    fn outsider_cannot_register() {
        let mut c = deploy();
        ctx(ALICE, 0, 0);
        assert!(matches!(
            c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn admin_controls_payment_authorities() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        c.add_payment_authority(acc(BOB)).unwrap();
        assert_eq!(c.get_payment_authorities(), vec![acc(BOB)]);
        c.remove_payment_authority(acc(BOB)).unwrap();
        assert!(c.get_payment_authorities().is_empty());
    }

    #[test]
    fn admin_controls_recovery_authorities() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        c.add_recovery_authority(acc(BOB)).unwrap();
        assert_eq!(c.get_recovery_authorities(), vec![acc(BOB)]);
        c.remove_recovery_authority(acc(BOB)).unwrap();
        assert!(c.get_recovery_authorities().is_empty());
    }

    #[test]
    fn only_the_council_grants_the_recovery_role() {
        let mut c = deploy();
        ctx(ALICE, 0, 0);
        assert!(matches!(
            c.add_recovery_authority(acc(BOB)),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn duplicate_registration_rejected() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 0, 0);
        assert!(matches!(
            c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None),
            Err(ContractError::TlaAlreadyRegistered)
        ));
    }

    #[test]
    fn business_tla_requires_licensee() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        assert!(matches!(
            c.register_tla(acc(TLA), TlaType::Business, PremiumCategory::Standard, None),
            Err(ContractError::BusinessTlaRequiresLicensee)
        ));
    }

    #[test]
    fn open_tla_rejects_business_activation_endpoint() {
        let mut c = deploy_priced();
        ctx(ADMIN, 0, 0);
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
        let fee = usd_to_near(
            c.get_fee_config().tla_allocation_fee_usd_micro.0
                + fees::base_rent(5, &c.get_fee_config()),
        );
        ctx(ADMIN, fee, 0);
        assert!(matches!(
            c.activate_tla(acc(TLA)),
            Err(ContractError::WrongActivationEndpoint)
        ));
    }

    #[test]
    fn suspend_blocks_rentals() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 0, 1);
        c.suspend_tla(acc(TLA)).unwrap();
        ctx(ALICE, rent_total(&c, "alice"), 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), None),
            Err(ContractError::TlaNotAcceptingRentals)
        ));
    }
}

mod rental {
    use super::*;

    #[test]
    fn rent_happy_path_records_entry() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let view = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(view.owner, acc(ALICE));
        assert!(matches!(view.lifecycle, LifecycleStatus::Active));
        assert_eq!(c.get_stats().sub_account_count, 1);
    }

    #[test]
    fn name_taken_rejected() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, rent_total(&c, "alice"), 2);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), None),
            Err(ContractError::SubAccountNameTaken)
        ));
    }

    #[test]
    fn underpayment_rejected() {
        let mut c = deploy_with_open_tla();
        let required =
            rent_near_open(&c, "alice") + c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ALICE, required - 1, 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), None),
            Err(ContractError::InsufficientPayment)
        ));
    }

    #[test]
    fn payout_defaults_to_the_renter() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let view = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(view.owner, acc(ALICE));
        assert_eq!(view.payout_account, acc(ALICE));
    }

    #[test]
    fn naming_another_owner_requires_payment_authority() {
        let mut c = deploy_with_open_tla();
        let total = rent_total(&c, "alice");
        ctx(BOB, total, 1);
        assert!(
            matches!(
                c.rent_sub_account(acc(TLA), "alice".to_string(), Some(acc(ALICE))),
                Err(ContractError::OnlyPaymentAuthority)
            ),
            "a stranger must not be able to rent a name into somebody else's control"
        );
    }

    #[test]
    fn a_sponsor_rents_a_name_that_belongs_to_the_user() {
        let mut c = deploy_with_open_tla();
        let rent_near = rent_near_open(&c, "alice");
        let total = rent_total(&c, "alice");
        ctx(ADMIN, 0, 1);
        c.add_payment_authority(acc(BOB)).unwrap();

        ctx(BOB, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), Some(acc(ALICE)))
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_created(
            settled("alice", ALICE, ALICE, rent_near, total),
            Ok(MintOutcome::Active),
        );

        let view = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(
            view.owner,
            acc(ALICE),
            "the sponsored lease belongs to the named user, not the sponsor"
        );
        assert_eq!(view.payout_account, acc(ALICE));
    }

    #[test]
    fn a_failed_sponsored_mint_refunds_the_sponsor_not_the_owner() {
        let mut c = deploy_with_open_tla();
        let rent_near = rent_near_open(&c, "alice");
        let total = rent_total(&c, "alice");
        ctx(ADMIN, 0, 1);
        c.add_payment_authority(acc(BOB)).unwrap();

        ctx(BOB, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), Some(acc(ALICE)))
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_sub_account_created(
            settled("alice", ALICE, BOB, rent_near, total),
            Err(PromiseError::Failed),
        );

        assert_eq!(
            c.get_pending_refund(acc(BOB)).0,
            total,
            "the sponsor paid, so the sponsor is refunded"
        );
        assert_eq!(
            c.get_pending_refund(acc(ALICE)).0,
            0,
            "the named owner never paid and must not be credited"
        );
    }

    #[test]
    fn a_sponsored_mint_records_the_owner_not_the_sponsor() {
        let mut c = deploy_with_open_tla();
        let rent_near = rent_near_open(&c, "alice");
        let total = rent_total(&c, "alice");
        ctx(ADMIN, 0, 1);
        c.add_payment_authority(acc(BOB)).unwrap();

        ctx(BOB, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), Some(acc(ALICE)))
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_created(
            settled("alice", ALICE, BOB, rent_near, total),
            Ok(MintOutcome::Active),
        );

        let view = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(view.owner, acc(ALICE));
        assert_eq!(view.payout_account, acc(ALICE));
    }

    #[test]
    fn renting_in_your_own_name_needs_no_authority() {
        let mut c = deploy_with_open_tla();
        let total = rent_total(&c, "alice");
        ctx(ALICE, total, 1);
        assert!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), Some(acc(ALICE)))
                .is_ok(),
            "naming yourself is the same as naming nobody"
        );
    }

    #[test]
    fn paid_rent_requires_authority_and_only_account_creation_deposit_yocto() {
        let mut c = deploy_with_open_tla();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;
        let rent = c
            .get_rent_price(acc(TLA), "alice".to_string())
            .unwrap()
            .rent_yocto
            .0;

        ctx(BOB, creation, 1);
        assert!(matches!(
            c.rent_sub_account_paid(acc(TLA), "alice".to_string(), acc(ALICE), acc(ALICE)),
            Err(ContractError::OnlyPaymentAuthority)
        ));

        ctx(ADMIN, 0, 1);
        c.add_payment_authority(acc(BOB)).unwrap();
        ctx(BOB, creation, 1);
        let _ = c
            .rent_sub_account_paid(acc(TLA), "alice".to_string(), acc(ALICE), acc(ALICE))
            .unwrap();
        c.on_sub_account_created_paid(
            settled("alice", ALICE, BOB, rent, creation),
            Ok(MintOutcome::Active),
        );

        let view = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(
            view.owner,
            acc(ALICE),
            "a sponsored mint belongs to the named owner, not to whoever paid"
        );
        assert_eq!(view.payout_account, acc(ALICE));
        assert_eq!(c.get_stats().total_revenue_yocto.0, 0);
        assert_eq!(c.get_pending_refund(acc(BOB)).0, 0);
    }

    #[test]
    fn failed_mint_refunds_payer_and_frees_name() {
        let mut c = deploy_with_open_tla();
        let total = rent_total(&c, "alice");
        ctx(ALICE, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), None)
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_sub_account_created(
            settled(
                "alice",
                ALICE,
                ALICE,
                total - c.get_fee_config().account_creation_deposit_yocto.0,
                total,
            ),
            Err(PromiseError::Failed),
        );
        assert_eq!(c.get_pending_refund(acc(ALICE)).0, total);
        assert!(c.is_name_available(acc(TLA), "alice".to_string()));
        assert_eq!(c.get_stats().sub_account_count, 0);
    }

    #[test]
    fn renewal_extends_expiry() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let before = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        let rent = c
            .get_rent_price(acc(TLA), "alice".to_string())
            .unwrap()
            .rent_yocto
            .0;
        ctx(ALICE, rent, 2);
        let _ = c.renew_sub_account(acc(TLA), "alice".to_string()).unwrap();
        let after = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        assert_eq!(after, before + ONE_YEAR_NS);
    }

    #[test]
    fn renewal_past_grace_rejected() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        let rent = c
            .get_rent_price(acc(TLA), "alice".to_string())
            .unwrap()
            .rent_yocto
            .0;
        ctx(ALICE, rent, expires + GRACE_NS + 1);
        assert!(matches!(
            c.renew_sub_account(acc(TLA), "alice".to_string()),
            Err(ContractError::SubAccountPastGracePeriod)
        ));
    }

    #[test]
    fn set_payout_account_owner_only() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 1, 2);
        assert!(matches!(
            c.set_payout_account(acc(TLA), "alice".to_string(), acc(BOB)),
            Err(ContractError::OnlyOwner)
        ));
        ctx(ALICE, 1, 2);
        c.set_payout_account(acc(TLA), "alice".to_string(), acc(BOB))
            .unwrap();
        assert_eq!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .payout_account,
            acc(BOB)
        );
    }
}

mod marketplace {
    use super::*;

    fn list_alice(c: &mut TlaRegistry, price: u128) {
        ctx(ALICE, 1, 2);
        c.list_sub_account(acc(TLA), "alice".to_string(), U128(price))
            .unwrap();
    }

    #[test]
    fn list_requires_owner() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 1, 2);
        assert!(matches!(
            c.list_sub_account(acc(TLA), "alice".to_string(), U128(10)),
            Err(ContractError::OnlyOwner)
        ));
    }

    #[test]
    fn list_and_unlist_roundtrip() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 10);
        assert_eq!(
            c.get_listing(acc(TLA), "alice".to_string())
                .unwrap()
                .price_yocto
                .0,
            10
        );
        ctx(ALICE, 1, 2);
        c.unlist_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert!(c.get_listing(acc(TLA), "alice".to_string()).is_none());
    }

    #[test]
    fn zero_price_rejected() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.list_sub_account(acc(TLA), "alice".to_string(), U128(0)),
            Err(ContractError::InvalidPrice)
        ));
    }

    #[test]
    fn buy_unlisted_rejected() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 10, 2);
        assert!(matches!(
            c.buy_sub_account(acc(TLA), "alice".to_string()),
            Err(ContractError::NotListed)
        ));
    }

    #[test]
    fn buy_below_price_rejected() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 10);
        ctx(BOB, 9, 2);
        assert!(matches!(
            c.buy_sub_account(acc(TLA), "alice".to_string()),
            Err(ContractError::PriceNotMet)
        ));
    }

    #[test]
    fn buy_rejects_dotted_name() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 10, 2);
        assert!(matches!(
            c.buy_sub_account(acc(TLA), "ali.ce".to_string()),
            Err(ContractError::InvalidName { .. })
        ));
    }

    #[test]
    fn every_name_entrypoint_rejects_a_dotted_name() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        assert!(
            matches!(
                c.set_payout_account(acc(TLA), "ali.ce".to_string(), acc(BOB)),
                Err(ContractError::InvalidName { .. })
            ),
            "a dotted name must not reach the storage key"
        );
        ctx(ALICE, 10, 2);
        assert!(matches!(
            c.renew_sub_account(acc(TLA), "ali.ce".to_string()),
            Err(ContractError::InvalidName { .. })
        ));
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.schedule_retraction(acc(TLA), "ali.ce".to_string()),
            Err(ContractError::InvalidName { .. })
        ));
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.cancel_retraction(acc(TLA), "ali.ce".to_string()),
            Err(ContractError::InvalidName { .. })
        ));
    }

    #[test]
    fn relisting_during_settlement_is_refused() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 10);
        ctx(BOB, 10, 2);
        let _ = c.buy_sub_account(acc(TLA), "alice".to_string()).unwrap();
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.list_sub_account(acc(TLA), "alice".to_string(), U128(20)),
            Err(ContractError::SaleInProgress)
        ));
        assert!(
            c.get_listing(acc(TLA), "alice".to_string())
                .unwrap()
                .settling
        );
    }

    #[test]
    fn reaccepting_during_settlement_is_refused() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        accept_alice(&mut c, acc(BOB), 10);
        ctx(BOB, 10, 2);
        let _ = c.buy_sub_account(acc(TLA), "alice".to_string()).unwrap();
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.accept_offer(acc(TLA), "alice".to_string(), acc(BOB), U128(10)),
            Err(ContractError::SaleInProgress)
        ));
        assert!(
            c.get_accepted_offer(acc(TLA), "alice".to_string())
                .unwrap()
                .settling
        );
    }

    #[test]
    fn listing_after_an_owner_change_is_refused() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = format!("alice.{TLA}");
        if let Some(sub) = c.sub_accounts.get_mut(&key) {
            sub.owner = acc(BOB);
        }
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.list_sub_account(acc(TLA), "alice".to_string(), U128(10)),
            Err(ContractError::OnlyOwner)
        ));
        assert!(c.get_listing(acc(TLA), "alice".to_string()).is_none());
    }

    #[test]
    fn buy_locks_sale_against_relist() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 10);
        ctx(BOB, 10, 2);
        let _ = c.buy_sub_account(acc(TLA), "alice".to_string()).unwrap();
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.unlist_sub_account(acc(TLA), "alice".to_string()),
            Err(ContractError::SaleInProgress)
        ));
    }

    #[test]
    fn sold_callback_pays_seller_and_swaps_owner() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 100);
        ctx(ADMIN, 0, 2);
        let mut config = c.get_fee_config();
        config.resale_commission_bps = 500;
        c.update_fee_config(config).unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_sold(
            acc(TLA),
            "alice".to_string(),
            acc(BOB),
            U128(100),
            U128(120),
            Ok(true),
        );
        assert_eq!(c.get_pending_refund(acc(ALICE)).0, 95);
        assert_eq!(c.get_pending_refund(acc(BOB)).0, 20);
        let view = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(view.owner, acc(BOB));
        assert!(c.get_listing(acc(TLA), "alice".to_string()).is_none());
    }

    #[test]
    fn sale_proceeds_follow_the_payout_account_not_the_owner() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        c.set_payout_account(acc(TLA), "alice".to_string(), acc(CAROL))
            .unwrap();
        list_alice(&mut c, 100);
        ctx(ADMIN, 0, 2);
        let mut config = c.get_fee_config();
        config.resale_commission_bps = 500;
        c.update_fee_config(config).unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_sold(
            acc(TLA),
            "alice".to_string(),
            acc(BOB),
            U128(100),
            U128(100),
            Ok(true),
        );
        assert_eq!(
            c.get_pending_refund(acc(CAROL)).0,
            95,
            "proceeds must go to the account the seller nominated"
        );
        assert_eq!(
            c.get_pending_refund(acc(ALICE)).0,
            0,
            "the owner account must not receive proceeds once a payout account is set"
        );
    }

    #[test]
    fn paid_buy_skips_near_proceeds_and_sets_payout_account_to_buyer() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 100);
        let revenue_before = c.get_stats().total_revenue_yocto.0;

        ctx(ADMIN, 0, 2);
        c.add_payment_authority(acc(BOB)).unwrap();
        ctx(BOB, 0, 2);
        let _ = c
            .buy_sub_account_paid(acc(TLA), "alice".to_string(), acc(CAROL))
            .unwrap();
        assert!(
            c.get_listing(acc(TLA), "alice".to_string())
                .unwrap()
                .settling
        );

        c.on_sub_account_sold_paid(
            acc(TLA),
            "alice".to_string(),
            acc(CAROL),
            acc(BOB),
            U128(100),
            Ok(true),
        );

        assert_eq!(c.get_stats().total_revenue_yocto.0, revenue_before);
        assert_eq!(c.get_pending_refund(acc(ALICE)).0, 0);
        assert_eq!(c.get_pending_refund(acc(CAROL)).0, 0);
        let view = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(view.owner, acc(BOB));
        assert_eq!(view.payout_account, acc(CAROL));
        assert!(c.get_listing(acc(TLA), "alice".to_string()).is_none());
    }

    #[test]
    fn failed_sale_refunds_buyer_and_unlocks() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 100);
        ctx(BOB, 100, 2);
        let _ = c.buy_sub_account(acc(TLA), "alice".to_string()).unwrap();
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_sub_account_sold(
            acc(TLA),
            "alice".to_string(),
            acc(BOB),
            U128(100),
            U128(100),
            Err(near_sdk::PromiseError::Failed),
        );
        assert_eq!(c.get_pending_refund(acc(BOB)).0, 100);
        assert_eq!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .owner,
            acc(ALICE)
        );
        ctx(ALICE, 1, 2);
        c.unlist_sub_account(acc(TLA), "alice".to_string()).unwrap();
    }

    #[test]
    fn voided_swap_refunds_buyer_and_unlocks() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 100);
        ctx(BOB, 100, 2);
        let _ = c.buy_sub_account(acc(TLA), "alice".to_string()).unwrap();
        c.on_sub_account_sold(
            acc(TLA),
            "alice".to_string(),
            acc(BOB),
            U128(100),
            U128(100),
            Ok(false),
        );
        assert_eq!(c.get_pending_refund(acc(BOB)).0, 100);
        assert_eq!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .owner,
            acc(ALICE)
        );
        ctx(ALICE, 1, 2);
        c.unlist_sub_account(acc(TLA), "alice".to_string()).unwrap();
    }

    fn accept_alice(c: &mut TlaRegistry, buyer: AccountId, price: u128) {
        ctx(ALICE, 1, 2);
        c.accept_offer(acc(TLA), "alice".to_string(), buyer, U128(price))
            .unwrap();
    }

    #[test]
    fn accepted_offer_settles_at_offer_price() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        accept_alice(&mut c, acc(BOB), 50);
        ctx(BOB, 50, 2);
        let _ = c.buy_sub_account(acc(TLA), "alice".to_string()).unwrap();
        let offer = c.get_accepted_offer(acc(TLA), "alice".to_string()).unwrap();
        assert!(offer.settling);
    }

    #[test]
    fn transfer_requires_owner() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 1, 2);
        assert!(matches!(
            c.transfer_sub_account(acc(TLA), "alice".to_string(), acc(BOB)),
            Err(ContractError::OnlyOwner)
        ));
    }

    #[test]
    fn transfer_requires_one_yocto() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 0, 2);
        assert!(matches!(
            c.transfer_sub_account(acc(TLA), "alice".to_string(), acc(BOB)),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn transfer_rejects_the_current_owner_as_recipient() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.transfer_sub_account(acc(TLA), "alice".to_string(), acc(ALICE)),
            Err(ContractError::SameOwner)
        ));
    }

    #[test]
    fn transfer_rejects_the_account_itself_as_recipient() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.transfer_sub_account(acc(TLA), "alice".to_string(), acc(&format!("alice.{TLA}"))),
            Err(ContractError::TransferToSubAccount)
        ));
    }

    #[test]
    fn transfer_is_locked_out_while_a_sale_settles() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_alice(&mut c, 10);
        ctx(BOB, 10, 2);
        let _ = c.buy_sub_account(acc(TLA), "alice".to_string()).unwrap();
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.transfer_sub_account(acc(TLA), "alice".to_string(), acc(BOB)),
            Err(ContractError::SaleInProgress)
        ));
    }

    #[test]
    fn transfer_rejects_a_caller_who_no_longer_owns_the_name() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = format!("alice.{TLA}");
        if let Some(sub) = c.sub_accounts.get_mut(&key) {
            sub.owner = acc(BOB);
        }
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.transfer_sub_account(acc(TLA), "alice".to_string(), acc(CAROL)),
            Err(ContractError::OnlyOwner)
        ));
        assert_eq!(
            c.sub_accounts.get(&key).map(|s| s.owner.clone()),
            Some(acc(BOB))
        );
    }

    #[test]
    fn transferred_callback_moves_owner_and_payout_account() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 0, 2);
        c.on_sub_account_transferred(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            Ok(true),
        );
        let key = format!("alice.{TLA}");
        let sub = c.sub_accounts.get(&key).unwrap();
        assert_eq!(sub.owner, acc(BOB));
        assert_eq!(sub.payout_account, acc(BOB));
    }

    #[test]
    fn transferred_callback_leaves_owner_untouched_when_the_swap_failed() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 0, 2);
        c.on_sub_account_transferred(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            Ok(false),
        );
        let key = format!("alice.{TLA}");
        assert_eq!(
            c.sub_accounts.get(&key).map(|s| s.owner.clone()),
            Some(acc(ALICE))
        );
    }
}

mod recovery {
    use super::*;

    fn deploy_with_recovery() -> TlaRegistry {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 0, 0);
        c.add_recovery_authority(acc(CAROL)).unwrap();
        c
    }

    #[test]
    fn a_stranger_cannot_recover_a_name() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 1, 2);
        assert!(matches!(
            c.recover_sub_account(acc(TLA), "alice".to_string(), acc(BOB)),
            Err(ContractError::OnlyRecoveryAuthority)
        ));
    }

    #[test]
    fn the_current_owner_cannot_recover_their_own_name() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.recover_sub_account(acc(TLA), "alice".to_string(), acc(BOB)),
            Err(ContractError::OnlyRecoveryAuthority)
        ));
    }

    #[test]
    fn recovery_hands_the_name_to_a_new_owner_account() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(CAROL, 1, 2);
        assert!(c
            .recover_sub_account(acc(TLA), "alice".to_string(), acc(BOB))
            .is_ok());
    }

    #[test]
    fn recovery_requires_one_yocto() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(CAROL, 0, 2);
        assert!(matches!(
            c.recover_sub_account(acc(TLA), "alice".to_string(), acc(BOB)),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn recovery_rejects_a_no_op_owner_change() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(CAROL, 1, 2);
        assert!(matches!(
            c.recover_sub_account(acc(TLA), "alice".to_string(), acc(ALICE)),
            Err(ContractError::SameOwner)
        ));
    }

    #[test]
    fn recovery_refuses_an_unknown_name() {
        let mut c = deploy_with_recovery();
        ctx(CAROL, 1, 2);
        assert!(matches!(
            c.recover_sub_account(acc(TLA), "nobody".to_string(), acc(BOB)),
            Err(ContractError::SubAccountNotFound)
        ));
    }

    #[test]
    fn recovery_callback_moves_owner_and_payout_and_clears_any_listing() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        c.list_sub_account(acc(TLA), "alice".to_string(), U128(1_000))
            .unwrap();
        ctx(CAROL, 0, 2);
        c.on_sub_account_recovered(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            Ok(true),
        );
        let key = format!("alice.{TLA}");
        let sub = c.sub_accounts.get(&key).unwrap();
        assert_eq!(sub.owner, acc(BOB));
        assert_eq!(sub.payout_account, acc(BOB));
        assert!(
            c.get_listing(acc(TLA), "alice".to_string()).is_none(),
            "a recovered name must not stay listed by the lost owner"
        );
    }

    #[test]
    fn recovery_callback_leaves_the_owner_untouched_when_the_swap_failed() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(CAROL, 0, 2);
        c.on_sub_account_recovered(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            Ok(false),
        );
        let key = format!("alice.{TLA}");
        assert_eq!(
            c.sub_accounts.get(&key).map(|s| s.owner.clone()),
            Some(acc(ALICE))
        );
    }
}

mod reclaim {
    use super::*;

    #[test]
    fn active_sub_not_reclaimable() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 0, 2);
        assert!(matches!(
            c.reclaim_finalize(acc(TLA), "alice".to_string()),
            Err(ContractError::SubAccountNotReclaimable)
        ));
    }

    #[test]
    fn reclaim_finalized_parks_name_and_rerent_works() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE));
        assert!(c.is_name_re_rentable(acc(TLA), "alice".to_string()));
        assert!(c.is_name_available(acc(TLA), "alice".to_string()));
        assert_eq!(c.get_stats().sub_account_count, 0);

        let rent = c
            .get_rent_price(acc(TLA), "alice".to_string())
            .unwrap()
            .rent_yocto
            .0;
        ctx(BOB, rent, 3);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), None)
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_re_rented(settled("alice", BOB, BOB, rent, rent));
        assert!(!c.is_name_re_rentable(acc(TLA), "alice".to_string()));
        assert_eq!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .owner,
            acc(BOB)
        );
    }

    #[test]
    fn expired_sub_is_reclaimable_lifecycle() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        ctx(BOB, 0, expires + GRACE_NS + DAY_NS);
        let view = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert!(matches!(view.lifecycle, LifecycleStatus::Reclaimable));
    }

    #[test]
    fn reclaim_finalize_rejected_while_sale_settling() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        ctx(ALICE, 1, 2);
        c.list_sub_account(acc(TLA), "alice".to_string(), U128(10))
            .unwrap();
        ctx(BOB, 10, 2);
        let _ = c.buy_sub_account(acc(TLA), "alice".to_string()).unwrap();
        ctx(BOB, 0, expires + GRACE_NS + DAY_NS);
        assert!(matches!(
            c.reclaim_finalize(acc(TLA), "alice".to_string()),
            Err(ContractError::SaleInProgress)
        ));
    }

    #[test]
    fn double_reclaim_does_not_double_decrement() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        assert_eq!(c.get_stats().sub_account_count, 1);
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE));
        assert_eq!(c.get_stats().sub_account_count, 0);
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE));
        assert_eq!(c.get_stats().sub_account_count, 0);
    }

    #[test]
    #[should_panic(expected = "grace period too short")]
    fn new_rejects_short_grace_period() {
        ctx(ADMIN, 0, 0);
        let _ = TlaRegistry::new(acc(ADMIN), acc(HOSEXT), U64(0), acc(TREASURY), acc(COUNCIL));
    }

    #[test]
    fn concurrent_reclaim_blocked_until_finalized() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        ctx(BOB, 0, expires + GRACE_NS + DAY_NS);
        let _ = c
            .reclaim_finalize(acc(TLA), "alice".to_string())
            .expect("first reclaim dispatches and takes the in-progress lock");
        assert!(matches!(
            c.reclaim_finalize(acc(TLA), "alice".to_string()),
            Err(ContractError::ReclaimInProgress)
        ));
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE));
        assert!(c.is_name_re_rentable(acc(TLA), "alice".to_string()));
    }

    #[test]
    fn admin_can_clear_reclaim_pending() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        ctx(BOB, 0, expires + GRACE_NS + DAY_NS);
        let _ = c
            .reclaim_finalize(acc(TLA), "alice".to_string())
            .expect("reclaim takes the in-progress lock");
        assert!(c.is_reclaim_in_progress(acc(TLA), "alice".to_string()));
        ctx(ADMIN, 0, 2);
        c.admin_clear_reclaim_pending(acc(TLA), "alice".to_string())
            .unwrap();
        assert!(!c.is_reclaim_in_progress(acc(TLA), "alice".to_string()));
    }

    #[test]
    fn a_pending_reclaim_blocks_listing_and_buying() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = format!("alice.{TLA}");
        c.reclaim_pending.insert(key, true);
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.list_sub_account(acc(TLA), "alice".to_string(), U128(10)),
            Err(ContractError::ReclaimInProgress)
        ));
        ctx(BOB, 10, 2);
        assert!(matches!(
            c.buy_sub_account(acc(TLA), "alice".to_string()),
            Err(ContractError::ReclaimInProgress)
        ));
    }

    #[test]
    fn stale_reclaim_callback_aborts_when_no_longer_reclaimable() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = format!("alice.{TLA}");
        c.reclaim_pending.insert(key.clone(), true);
        testing_env!(VMContextBuilder::new()
            .current_account_id(acc("registry.testnet"))
            .predecessor_account_id(acc("registry.testnet"))
            .block_timestamp(2)
            .build());
        let out = c.on_balances_checked(acc(TLA), "alice".to_string(), acc(ALICE), vec![]);
        assert!(matches!(out, near_sdk::PromiseOrValue::Value(())));
        assert!(!c.reclaim_pending.contains_key(&key));
        assert!(c.get_sub_account(acc(TLA), "alice".to_string()).is_some());
    }
}

mod refunds_and_admin {
    use super::*;

    #[test]
    fn claim_refund_requires_pending() {
        let mut c = deploy();
        ctx(ALICE, 0, 1);
        assert!(matches!(
            c.claim_refund(),
            Err(ContractError::NoPendingRefund)
        ));
    }

    #[test]
    fn withdraw_capped_by_revenue() {
        let mut c = deploy();
        ctx(ADMIN, 0, 1);
        assert!(matches!(
            c.withdraw(U128(1)),
            Err(ContractError::InsufficientRevenue)
        ));
    }

    #[test]
    fn pause_blocks_rent() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 0, 1);
        c.pause().unwrap();
        ctx(ALICE, rent_total(&c, "alice"), 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), None),
            Err(ContractError::Paused)
        ));
    }

    #[test]
    fn allowlist_roundtrip() {
        let mut c = deploy();
        ctx(ADMIN, 0, 1);
        c.add_ft_allowlist(acc("token.testnet")).unwrap();
        assert_eq!(c.get_ft_allowlist(), vec![acc("token.testnet")]);
        c.remove_ft_allowlist(acc("token.testnet")).unwrap();
        assert!(c.get_ft_allowlist().is_empty());
    }

    #[test]
    fn fee_config_guards() {
        let mut c = deploy();
        ctx(ADMIN, 0, 1);
        let mut config = c.get_fee_config();
        config.resale_commission_bps = 10_001;
        assert!(matches!(
            c.update_fee_config(config),
            Err(ContractError::InvalidCommissionRate)
        ));
    }
}

mod business {
    use super::*;

    fn deploy_with_business_tla() -> TlaRegistry {
        let mut c = deploy_priced();
        ctx(ADMIN, 0, 0);
        c.register_tla(
            acc(TLA),
            TlaType::Business,
            PremiumCategory::Standard,
            Some(acc(ALICE)),
        )
        .unwrap();
        let fee = usd_to_near(
            c.get_fee_config().tla_allocation_fee_usd_micro.0
                + fees::base_rent(5, &c.get_fee_config()),
        );
        ctx(ALICE, fee, 0);
        c.activate_tla(acc(TLA)).unwrap();
        c
    }

    fn rent_business_sub(c: &mut TlaRegistry, name: &str) {
        let rent_near = usd_to_near(c.get_fee_config().sub_fee_per_account_usd_micro.0);
        let total = rent_near + c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ALICE, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), name.to_string(), None)
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_created(
            settled(name, ALICE, ALICE, rent_near, total),
            Ok(MintOutcome::Active),
        );
    }

    #[test]
    fn only_licensee_rents_business_subs() {
        let mut c = deploy_with_business_tla();
        let total = c.get_fee_config().sub_fee_per_account_usd_micro.0
            + c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(BOB, total, 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "staff".to_string(), None),
            Err(ContractError::OnlyLicensee)
        ));
    }

    #[test]
    fn business_cap_view_reflects_override_then_falls_back() {
        let mut c = deploy_with_business_tla();
        let default_cap = c.get_fee_config().business_max_subs;
        assert_eq!(c.get_business_sub_cap(acc(TLA)), default_cap);
        assert_eq!(c.get_business_sub_count(acc(TLA)), 0);
        ctx(ADMIN, 0, 1);
        c.set_business_sub_cap(acc(TLA), Some(7)).unwrap();
        assert_eq!(c.get_business_sub_cap(acc(TLA)), 7);
        ctx(ADMIN, 0, 1);
        c.set_business_sub_cap(acc(TLA), None).unwrap();
        assert_eq!(
            c.get_business_sub_cap(acc(TLA)),
            default_cap,
            "clearing the override must fall back to the configured default"
        );
    }

    #[test]
    fn business_renewal_cost_quotes_the_tla_rent_only() {
        let c = deploy_with_business_tla();
        let view = c.get_business_renewal_cost(acc(TLA)).unwrap();
        assert_eq!(view.tla_id, acc(TLA));
        assert_eq!(view.sub_count, 0);
        assert!(view.tla_rent_yocto.0 > 0);
        assert!(
            view.per_sub_yocto.0 > 0,
            "per-sub cost is reported separately because renew_tla does not charge it"
        );
    }

    #[test]
    fn business_renewal_cost_rejects_an_open_tla() {
        let c = deploy_with_open_tla();
        assert!(matches!(
            c.get_business_renewal_cost(acc(TLA)),
            Err(ContractError::NotBusinessTla)
        ));
    }

    #[test]
    fn business_cap_enforced() {
        let mut c = deploy_with_business_tla();
        ctx(ADMIN, 0, 1);
        c.set_business_sub_cap(acc(TLA), Some(0)).unwrap();
        let total = usd_to_near(c.get_fee_config().sub_fee_per_account_usd_micro.0)
            + c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ALICE, total, 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "staff".to_string(), None),
            Err(ContractError::MaxBusinessSubsReached)
        ));
    }

    #[test]
    fn retraction_schedule_and_cancel() {
        let mut c = deploy_with_business_tla();
        let rent_near = usd_to_near(c.get_fee_config().sub_fee_per_account_usd_micro.0);
        let total = rent_near + c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ALICE, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "staff".to_string(), None)
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_created(
            settled("staff", ALICE, ALICE, rent_near, total),
            Ok(MintOutcome::Active),
        );
        ctx(ALICE, 1, 2);
        c.schedule_retraction(acc(TLA), "staff".to_string())
            .unwrap();
        assert!(c.get_retraction_at(acc(TLA), "staff".to_string()).is_some());
        ctx(ALICE, 1, 3);
        c.cancel_retraction(acc(TLA), "staff".to_string()).unwrap();
        assert!(c.get_retraction_at(acc(TLA), "staff".to_string()).is_none());
    }

    #[test]
    fn business_sub_cannot_be_listed() {
        let mut c = deploy_with_business_tla();
        rent_business_sub(&mut c, "staff");
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.list_sub_account(acc(TLA), "staff".to_string(), U128(10)),
            Err(ContractError::BusinessSubNotResellable)
        ));
    }

    #[test]
    fn business_sub_cannot_be_bought() {
        let mut c = deploy_with_business_tla();
        rent_business_sub(&mut c, "staff");
        ctx(BOB, 10, 2);
        assert!(matches!(
            c.buy_sub_account(acc(TLA), "staff".to_string()),
            Err(ContractError::BusinessSubNotResellable)
        ));
    }

    #[test]
    fn business_sub_offer_cannot_be_accepted() {
        let mut c = deploy_with_business_tla();
        rent_business_sub(&mut c, "staff");
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.accept_offer(acc(TLA), "staff".to_string(), acc(BOB), U128(10)),
            Err(ContractError::BusinessSubNotResellable)
        ));
    }
}

mod price_oracle {
    use super::*;

    const KEEPER: &str = "keeper.testnet";
    const COOLDOWN: u64 = 300 * 1_000_000_000;

    fn dollars(d: u128) -> u128 {
        d * 1_000_000
    }

    fn deploy_initialized() -> TlaRegistry {
        let mut c = deploy();
        ctx(ADMIN, 0, 1);
        c.admin_set_initial_rate(U128(dollars(5))).unwrap();
        ctx(ADMIN, 0, 1);
        c.set_price_oracle(acc(KEEPER)).unwrap();
        c
    }

    #[test]
    fn oracle_defaults_to_admin_and_rate_starts_unset() {
        let c = deploy();
        assert_eq!(c.get_price_oracle(), acc(ADMIN));
        assert_eq!(c.get_near_usd_rate().0, 0);
        assert_eq!(c.get_rate_meta().sequence.0, 0);
    }

    #[test]
    fn admin_initializes_rate_and_stamps_sequence() {
        let c = deploy_initialized();
        assert_eq!(c.get_near_usd_rate().0, dollars(5));
        let meta = c.get_rate_meta();
        assert_eq!(meta.updated_at.0, 1);
        assert_eq!(meta.sequence.0, 1);
    }

    #[test]
    fn keeper_cannot_bootstrap_uninitialized_rate() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        c.set_price_oracle(acc(KEEPER)).unwrap();
        ctx(KEEPER, 0, 1);
        assert!(matches!(
            c.set_near_usd_rate(U128(dollars(5))),
            Err(ContractError::RateNotInitialized)
        ));
    }

    #[test]
    fn initial_rate_rejected_outside_absolute_bounds() {
        let mut c = deploy();
        ctx(ADMIN, 0, 1);
        assert!(matches!(
            c.admin_set_initial_rate(U128(dollars(1_000))),
            Err(ContractError::RateOutOfBounds)
        ));
    }

    #[test]
    fn initial_rate_cannot_be_set_twice() {
        let mut c = deploy_initialized();
        ctx(ADMIN, 0, 1);
        assert!(matches!(
            c.admin_set_initial_rate(U128(dollars(6))),
            Err(ContractError::RateAlreadyInitialized)
        ));
    }

    #[test]
    fn keeper_moves_rate_within_band_after_cooldown() {
        let mut c = deploy_initialized();
        ctx(KEEPER, 0, 1 + COOLDOWN);
        c.set_near_usd_rate(U128(dollars(5) * 11_000 / 10_000))
            .unwrap();
        assert_eq!(c.get_near_usd_rate().0, dollars(5) * 11_000 / 10_000);
        assert_eq!(c.get_rate_meta().sequence.0, 2);
    }

    #[test]
    fn keeper_update_before_cooldown_rejected() {
        let mut c = deploy_initialized();
        ctx(KEEPER, 0, 1 + COOLDOWN - 1);
        assert!(matches!(
            c.set_near_usd_rate(U128(dollars(5) * 11_000 / 10_000)),
            Err(ContractError::RateCooldown)
        ));
    }

    #[test]
    fn non_oracle_cannot_set_rate() {
        let mut c = deploy_initialized();
        ctx(ADMIN, 0, 1 + COOLDOWN);
        assert!(matches!(
            c.set_near_usd_rate(U128(dollars(5))),
            Err(ContractError::OnlyPriceOracle)
        ));
    }

    #[test]
    fn rate_move_beyond_band_rejected() {
        let mut c = deploy_initialized();
        ctx(KEEPER, 0, 1 + COOLDOWN);
        assert!(matches!(
            c.set_near_usd_rate(U128(dollars(5) * 12_001 / 10_000)),
            Err(ContractError::RateOutOfBounds)
        ));
    }

    #[test]
    fn compromised_keeper_cannot_zero_or_floor_the_rate() {
        let mut c = deploy_initialized();
        ctx(KEEPER, 0, 1 + COOLDOWN);
        assert!(matches!(
            c.set_near_usd_rate(U128(0)),
            Err(ContractError::RateOutOfBounds)
        ));
        ctx(KEEPER, 0, 1 + COOLDOWN);
        assert!(matches!(
            c.set_near_usd_rate(U128(1)),
            Err(ContractError::RateOutOfBounds)
        ));
        assert_eq!(c.get_near_usd_rate().0, dollars(5));
    }

    #[test]
    fn ratchet_to_ceiling_is_bounded_by_absolute_max() {
        let mut c = deploy_initialized();
        let mut ts = 1 + COOLDOWN;
        let mut rate = dollars(5);
        for _ in 0..200 {
            let target = (rate * 12_000 / 10_000).min(dollars(100));
            ctx(KEEPER, 0, ts);
            if c.set_near_usd_rate(U128(target)).is_err() {
                break;
            }
            rate = target;
            ts += COOLDOWN;
        }
        assert!(c.get_near_usd_rate().0 <= dollars(100));
        assert_eq!(c.get_near_usd_rate().0, dollars(100));
    }

    #[test]
    fn a_stale_rate_stops_pricing() {
        let mut c = deploy_initialized();
        ctx(ADMIN, 0, 1);
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
        let max_age = c.get_fee_config().max_rate_age_ns.0;
        ctx(ALICE, 0, 1 + max_age);
        assert!(
            c.get_rent_price(acc(TLA), "alice".to_string()).is_ok(),
            "a rate exactly at the age limit is still usable"
        );
        ctx(ALICE, 0, 2 + max_age);
        assert!(matches!(
            c.get_rent_price(acc(TLA), "alice".to_string()),
            Err(ContractError::RateStale)
        ));
    }

    #[test]
    fn pricing_rejects_a_stored_rate_outside_the_configured_band() {
        let mut c = deploy_initialized();
        ctx(ADMIN, 0, 1);
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
        c.fee_config.max_near_usd_rate_micro = U128(dollars(2));
        ctx(ALICE, 0, 2);
        assert!(
            matches!(
                c.get_rent_price(acc(TLA), "alice".to_string()),
                Err(ContractError::RateOutOfBounds)
            ),
            "a stored rate must be re-checked against the band at point of use"
        );
    }

    #[test]
    fn the_oracle_can_recover_a_rate_stranded_outside_the_band() {
        let mut c = deploy_initialized();
        c.fee_config.max_near_usd_rate_micro = U128(dollars(2));
        ctx(KEEPER, 0, 1 + COOLDOWN);
        c.set_near_usd_rate(U128(dollars(2)))
            .expect("an out-of-band baseline must not veto a move back inside the band");
        assert_eq!(c.get_near_usd_rate().0, dollars(2));
        assert!(
            near_sdk::test_utils::get_logs()
                .iter()
                .any(|l| l.contains("rate_recovered_from_out_of_band")),
            "recovering from a stranded rate must be observable, not silent"
        );
    }

    #[test]
    fn an_ordinary_rate_move_is_not_reported_as_a_recovery() {
        let mut c = deploy_initialized();
        ctx(KEEPER, 0, 1 + COOLDOWN);
        c.set_near_usd_rate(U128(dollars(5) * 11_000 / 10_000))
            .unwrap();
        assert!(
            !near_sdk::test_utils::get_logs()
                .iter()
                .any(|l| l.contains("rate_recovered_from_out_of_band")),
            "the recovery signal must stay rare enough to mean something"
        );
    }

    #[test]
    fn config_cannot_strand_the_live_rate_outside_the_new_band() {
        let mut c = deploy_initialized();
        let mut config = c.get_fee_config();
        config.min_near_usd_rate_micro = U128(dollars(1));
        config.max_near_usd_rate_micro = U128(dollars(2));
        ctx(ADMIN, 0, 1);
        assert!(
            matches!(
                c.update_fee_config(config),
                Err(ContractError::RateOutOfBounds)
            ),
            "a band that excludes the live rate halts pricing with no oracle move able to recover"
        );
    }

    #[test]
    fn config_rejects_unsafe_operational_values() {
        let mut c = deploy();
        let base = c.get_fee_config();

        let mut zero_cap = base.clone();
        zero_cap.business_max_subs = 0;
        ctx(ADMIN, 0, 1);
        assert!(matches!(
            c.update_fee_config(zero_cap),
            Err(ContractError::InvalidBusinessCap)
        ));

        let mut short_notice = base.clone();
        short_notice.retraction_notice_ns = U64(1);
        ctx(ADMIN, 0, 1);
        assert!(matches!(
            c.update_fee_config(short_notice),
            Err(ContractError::RetractionNoticeTooShort)
        ));

        let mut age_below_cooldown = base.clone();
        age_below_cooldown.max_rate_age_ns = U64(1);
        ctx(ADMIN, 0, 1);
        assert!(matches!(
            c.update_fee_config(age_below_cooldown),
            Err(ContractError::InvalidRateBounds)
        ));

        let mut no_cooldown = base.clone();
        no_cooldown.rate_update_cooldown_ns = U64(0);
        ctx(ADMIN, 0, 1);
        assert!(matches!(
            c.update_fee_config(no_cooldown),
            Err(ContractError::InvalidRateBounds)
        ));

        let mut huge_fee = base;
        huge_fee.rent_tier_5_usd_micro = U128(crate::pricing::MAX_USD_MICRO + 1);
        ctx(ADMIN, 0, 1);
        assert!(matches!(
            c.update_fee_config(huge_fee),
            Err(ContractError::FeeExceedsCap)
        ));
    }

    #[test]
    fn config_rejects_inverted_rate_bounds() {
        let mut c = deploy();
        ctx(ADMIN, 0, 1);
        let mut config = c.get_fee_config();
        config.min_near_usd_rate_micro = U128(dollars(50));
        config.max_near_usd_rate_micro = U128(dollars(10));
        assert!(matches!(
            c.update_fee_config(config),
            Err(ContractError::InvalidRateBounds)
        ));
    }

    #[test]
    fn config_rejects_out_of_range_bps() {
        let mut c = deploy();
        ctx(ADMIN, 0, 1);
        let mut config = c.get_fee_config();
        config.max_rate_move_bps = 10_001;
        assert!(matches!(
            c.update_fee_config(config),
            Err(ContractError::InvalidRateBounds)
        ));
    }

    #[test]
    fn quote_is_buffered_and_charge_refunds_the_buffer() {
        let mut c = deploy_with_open_tla();
        let rent_near = rent_near_open(&c, "alice");
        let deposit = c.get_fee_config().account_creation_deposit_yocto.0;
        let quote = c.get_rent_price(acc(TLA), "alice".to_string()).unwrap();
        assert!(
            quote.rent_yocto.0 > rent_near,
            "quote must carry the slippage buffer above the exact charge"
        );
        assert_eq!(quote.rent_yocto.0, rent_near * 10_527 / 10_000);
        assert_eq!(quote.total_yocto.0, quote.rent_yocto.0 + deposit);

        let attached = quote.total_yocto.0;
        ctx(ALICE, attached, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), None)
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_created(
            settled("alice", ALICE, ALICE, rent_near, attached),
            Ok(MintOutcome::Active),
        );
        assert_eq!(c.get_stats().total_revenue_yocto.0, rent_near);
        assert_eq!(
            c.get_pending_refund(acc(ALICE)).0,
            attached - rent_near - deposit
        );
    }

    #[test]
    fn charge_fails_closed_when_rate_uninitialized() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
        c.activate_open_tla(acc(TLA)).unwrap();
        assert!(matches!(
            c.get_rent_price(acc(TLA), "alice".to_string()),
            Err(ContractError::RateNotInitialized)
        ));
        ctx(ALICE, NearToken::from_near(100).as_yoctonear(), 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), None),
            Err(ContractError::RateNotInitialized)
        ));
    }
}

mod council_split {
    use super::*;

    fn deploy_split() -> TlaRegistry {
        ctx(ADMIN, 0, 0);
        TlaRegistry::new(
            acc(ADMIN),
            acc(HOSEXT),
            U64(GRACE_NS),
            acc(TREASURY),
            acc(OTHER_COUNCIL),
        )
    }

    #[test]
    fn an_operations_admin_cannot_escalate_itself() {
        let mut c = deploy_split();
        ctx(ADMIN, 0, 0);
        assert!(matches!(
            c.add_admin(acc(BOB)),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn an_operations_admin_cannot_move_the_fee_model_or_release_revenue() {
        let mut c = deploy_split();
        ctx(ADMIN, 0, 0);
        let config = c.get_fee_config();
        assert!(matches!(
            c.update_fee_config(config),
            Err(ContractError::OnlyCouncil)
        ));
        assert!(matches!(
            c.withdraw(U128(1)),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn an_operations_admin_cannot_grant_authorities_or_open_a_tla() {
        let mut c = deploy_split();
        ctx(ADMIN, 0, 0);
        assert!(matches!(
            c.add_payment_authority(acc(BOB)),
            Err(ContractError::OnlyCouncil)
        ));
        assert!(matches!(
            c.add_recovery_authority(acc(BOB)),
            Err(ContractError::OnlyCouncil)
        ));
        assert!(matches!(
            c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn the_council_holds_those_powers() {
        let mut c = deploy_split();
        ctx(OTHER_COUNCIL, 0, 0);
        c.add_admin(acc(BOB)).unwrap();
        c.add_payment_authority(acc(BOB)).unwrap();
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
    }

    #[test]
    fn operations_keeps_pause_and_unpause_so_an_incident_needs_no_multisig() {
        let mut c = deploy_split();
        ctx(ADMIN, 0, 0);
        c.pause().unwrap();
        assert!(c.is_paused());
        c.unpause().unwrap();
        assert!(!c.is_paused());
    }

    #[test]
    fn the_council_cannot_run_operations() {
        let mut c = deploy_split();
        ctx(OTHER_COUNCIL, 0, 0);
        assert!(matches!(c.pause(), Err(ContractError::OnlyAdmin)));
    }
}

mod marketplace_pause {
    use super::*;

    fn pause_market(c: &mut TlaRegistry) {
        ctx(ADMIN, 0, 2);
        c.pause_marketplace().unwrap();
    }

    #[test]
    fn a_paused_marketplace_refuses_listing() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        pause_market(&mut c);
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.list_sub_account(acc(TLA), "alice".to_string(), U128(10)),
            Err(ContractError::MarketplacePaused)
        ));
    }

    #[test]
    fn a_paused_marketplace_refuses_buying() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        pause_market(&mut c);
        ctx(BOB, 10, 3);
        assert!(matches!(
            c.buy_sub_account(acc(TLA), "alice".to_string()),
            Err(ContractError::MarketplacePaused)
        ));
    }

    #[test]
    fn renting_still_works_while_the_marketplace_is_paused() {
        let mut c = deploy_with_open_tla();
        pause_market(&mut c);
        rent_alice_sub(&mut c, "alice");
        assert!(c.get_sub_account(acc(TLA), "alice".to_string()).is_some());
    }

    #[test]
    fn unpausing_restores_listing() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        pause_market(&mut c);
        ctx(ADMIN, 0, 2);
        c.unpause_marketplace().unwrap();
        ctx(ALICE, 1, 2);
        assert!(c
            .list_sub_account(acc(TLA), "alice".to_string(), U128(10))
            .is_ok());
    }

    #[test]
    fn the_marketplace_pause_is_separate_from_the_registry_pause() {
        let mut c = deploy_with_open_tla();
        pause_market(&mut c);
        assert!(c.is_marketplace_paused());
        assert!(!c.is_paused());
    }

    #[test]
    fn only_an_admin_can_pause_the_marketplace() {
        let mut c = deploy_with_open_tla();
        ctx(ALICE, 0, 2);
        assert!(matches!(
            c.pause_marketplace(),
            Err(ContractError::OnlyAdmin)
        ));
    }
}

mod a_pause_never_costs_a_user_their_name {
    use super::*;

    const WEEK_NS: u64 = 7 * 24 * 60 * 60 * 1_000_000_000;

    fn refresh_rate(c: &mut TlaRegistry, at: u64) {
        ctx(ADMIN, 0, at);
        c.set_near_usd_rate(U128(NEAR_USD_MICRO)).unwrap();
    }

    fn expired_and_paused(name: &str) -> (TlaRegistry, u64) {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, name);
        let expires = c
            .get_sub_account(acc(TLA), name.to_string())
            .unwrap()
            .expires_at
            .0;
        ctx(ADMIN, 0, expires + GRACE_NS);
        c.pause().unwrap();
        (c, expires)
    }

    fn status_of(c: &TlaRegistry, name: &str) -> LifecycleStatus {
        c.get_sub_account(acc(TLA), name.to_string())
            .unwrap()
            .lifecycle
    }

    #[test]
    fn a_name_cannot_lapse_while_the_contract_is_paused() {
        let (c, expires) = expired_and_paused("alice");
        ctx(ALICE, 0, expires + GRACE_NS + 1);
        assert!(
            matches!(status_of(&c, "alice"), LifecycleStatus::Grace),
            "a pause must hold a name in grace rather than let it become reclaimable"
        );
    }

    #[test]
    fn a_holder_gets_a_fresh_grace_window_after_the_pause_lifts() {
        let (mut c, expires) = expired_and_paused("alice");
        let long_after = expires + GRACE_NS + 2;
        ctx(ADMIN, 0, long_after);
        c.unpause().unwrap();
        ctx(ALICE, 0, long_after + 1);
        assert!(
            matches!(status_of(&c, "alice"), LifecycleStatus::Grace),
            "the grace window must restart when the pause lifts"
        );
        ctx(ALICE, 0, long_after + GRACE_NS + 2);
        assert!(matches!(
            status_of(&c, "alice"),
            LifecycleStatus::Reclaimable
        ));
    }

    #[test]
    fn a_pause_lapses_by_itself_at_the_ceiling() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 0, 1);
        c.pause().unwrap();
        assert!(c.is_paused());
        ctx(ALICE, 0, 1 + WEEK_NS + 1);
        assert!(
            !c.is_paused(),
            "an admin must not be able to hold a pause open indefinitely"
        );
    }

    #[test]
    fn a_suspension_lapses_by_itself_at_the_ceiling() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 0, 1);
        c.suspend_tla(acc(TLA)).unwrap();
        ctx(BOB, rent_total(&c, "bob"), 2);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "bob".to_string(), None),
            Err(ContractError::TlaNotAcceptingRentals)
        ));
        let after = 1 + WEEK_NS + 1;
        refresh_rate(&mut c, after);
        ctx(BOB, rent_total(&c, "bob"), after);
        assert!(
            c.rent_sub_account(acc(TLA), "bob".to_string(), None)
                .is_ok(),
            "a suspension must not block a namespace indefinitely"
        );
    }
}

mod a_pause_never_traps_a_user {
    use super::*;

    #[test]
    fn a_holder_can_still_renew_and_keep_their_name() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let rent = c
            .get_rent_price(acc(TLA), "alice".to_string())
            .unwrap()
            .rent_yocto
            .0;
        ctx(ADMIN, 0, 2);
        c.pause().unwrap();
        ctx(ALICE, rent, 2);
        assert!(
            c.renew_sub_account(acc(TLA), "alice".to_string()).is_ok(),
            "a pause must never let a name lapse that its holder is paying to keep"
        );
    }

    #[test]
    fn a_holder_can_still_sweep_their_own_balance_home() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        ctx(ADMIN, 0, 2);
        c.pause().unwrap();
        ctx(ALICE, 1, expires + GRACE_NS + 1);
        assert!(
            c.reclaim_sweep_near(acc(TLA), "alice".to_string()).is_ok(),
            "a pause must never hold a user's own balance in an expired account"
        );
    }

    #[test]
    fn a_delisted_token_is_still_sweepable_home() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let token = acc("usdc.testnet");
        ctx(ADMIN, 0, 1);
        c.add_ft_allowlist(token.clone()).unwrap();
        c.remove_ft_allowlist(token.clone()).unwrap();
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        ctx(
            ALICE,
            crate::reclaim::SWEEP_ATTACHED_REQUIRED.as_yoctonear(),
            expires + 1,
        );
        assert!(
            c.reclaim_sweep_ft(acc(TLA), "alice".to_string(), token)
                .is_ok(),
            "de-listing a token must not strand balances already held in accounts"
        );
    }

    #[test]
    fn a_pause_still_stops_new_rentals() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 0, 1);
        c.pause().unwrap();
        ctx(BOB, rent_total(&c, "bob"), 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "bob".to_string(), None),
            Err(ContractError::Paused)
        ));
    }
}

#[test]
fn a_seller_can_still_unlist_while_the_marketplace_is_paused() {
    let mut c = deploy_with_open_tla();
    rent_alice_sub(&mut c, "alice");
    ctx(ALICE, 1, 2);
    c.list_sub_account(acc(TLA), "alice".to_string(), U128(10))
        .unwrap();
    ctx(ADMIN, 0, 2);
    c.pause_marketplace().unwrap();
    ctx(ALICE, 1, 3);
    assert!(c.unlist_sub_account(acc(TLA), "alice".to_string()).is_ok());
}

mod paged_views {
    use super::*;
    use crate::ACTIVITY_CAPACITY;

    fn names(page: Vec<SubAccountDetailView>) -> Vec<String> {
        page.into_iter().map(|d| d.sub_account.full_name).collect()
    }

    fn owned(c: &TlaRegistry, who: &str) -> Vec<String> {
        names(c.list_sub_accounts_by_owner(acc(who), 0, 100))
    }

    fn list_at(c: &mut TlaRegistry, name: &str, price: u128) {
        ctx(ALICE, 1, 2);
        c.list_sub_account(acc(TLA), name.to_string(), U128(price))
            .unwrap();
    }

    #[test]
    fn sub_accounts_page_and_carry_their_name_and_tla() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "bob");
        let page = names(c.list_sub_accounts(0, 10));
        assert_eq!(page.len(), 2);
        assert!(page.contains(&format!("alice.{TLA}")));
        assert!(page.contains(&format!("bob.{TLA}")));
    }

    #[test]
    fn sub_account_paging_respects_offset_and_limit() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "bob");
        assert_eq!(c.list_sub_accounts(0, 1).len(), 1);
        assert_eq!(c.list_sub_accounts(1, 10).len(), 1);
        assert_eq!(c.list_sub_accounts(2, 10).len(), 0);
    }

    #[test]
    fn listings_are_enumerable() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_at(&mut c, "alice", 10);
        let page = c.list_listings(0, 10);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].full_name, format!("alice.{TLA}"));
        assert_eq!(page[0].price_yocto.0, 10);
    }

    #[test]
    fn an_unlisted_name_leaves_the_listing_page() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_at(&mut c, "alice", 10);
        ctx(ALICE, 1, 3);
        c.unlist_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(c.list_listings(0, 10).len(), 0);
    }

    #[test]
    fn empty_registry_pages_are_empty_not_missing() {
        let c = deploy_with_open_tla();
        assert_eq!(c.list_sub_accounts(0, 10).len(), 0);
        assert_eq!(c.list_listings(0, 10).len(), 0);
        assert!(owned(&c, ALICE).is_empty());
    }

    #[test]
    fn a_page_carries_the_market_state_so_no_second_call_is_needed() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_at(&mut c, "alice", 250);
        let page = c.list_sub_accounts(0, 10);
        let listing = page[0]
            .listing
            .as_ref()
            .expect("listed name carries listing");
        assert_eq!(listing.price_yocto.0, 250);
        assert!(!listing.settling);
        assert!(page[0].accepted_offer.is_none());
        assert!(page[0].retraction_at.is_none());
    }

    #[test]
    fn a_page_carries_an_accepted_offer() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        c.accept_offer(acc(TLA), "alice".to_string(), acc(BOB), U128(400))
            .unwrap();
        let page = c.list_sub_accounts(0, 10);
        let offer = page[0].accepted_offer.as_ref().expect("offer is carried");
        assert_eq!(offer.buyer, acc(BOB));
        assert_eq!(offer.price_yocto.0, 400);
    }

    #[test]
    fn renting_indexes_the_name_under_its_owner() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        assert_eq!(owned(&c, ALICE), vec![format!("alice.{TLA}")]);
        assert!(owned(&c, BOB).is_empty());
    }

    #[test]
    fn a_sponsored_rent_indexes_the_named_owner_not_the_payer() {
        let mut c = deploy_with_open_tla();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;
        let rent = c
            .get_rent_price(acc(TLA), "alice".to_string())
            .unwrap()
            .rent_yocto
            .0;
        ctx(ADMIN, 0, 1);
        c.add_payment_authority(acc(BOB)).unwrap();
        ctx(BOB, creation, 1);
        let _ = c
            .rent_sub_account_paid(acc(TLA), "alice".to_string(), acc(ALICE), acc(ALICE))
            .unwrap();
        c.on_sub_account_created_paid(
            settled("alice", ALICE, BOB, rent, creation),
            Ok(MintOutcome::Active),
        );
        assert_eq!(owned(&c, ALICE), vec![format!("alice.{TLA}")]);
        assert!(owned(&c, BOB).is_empty());
    }

    #[test]
    fn a_failed_mint_leaves_nothing_in_the_index() {
        let mut c = deploy_with_open_tla();
        let rent = rent_near_open(&c, "alice");
        let deposit = c.get_fee_config().account_creation_deposit_yocto.0;
        let total = rent + deposit;
        ctx(ALICE, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), None)
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_created(
            settled("alice", ALICE, ALICE, rent, total),
            Ok(MintOutcome::CreationFailed),
        );
        assert!(owned(&c, ALICE).is_empty());
    }

    #[test]
    fn reclaiming_a_name_drops_it_from_the_index() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE));
        assert!(owned(&c, ALICE).is_empty());
        assert!(c.list_sub_accounts(0, 10).is_empty());
    }

    #[test]
    fn transferring_moves_the_name_between_owners() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 0, 2);
        c.on_sub_account_transferred(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            Ok(true),
        );
        assert!(owned(&c, ALICE).is_empty());
        assert_eq!(owned(&c, BOB), vec![format!("alice.{TLA}")]);
    }

    #[test]
    fn a_failed_transfer_leaves_the_index_with_the_original_owner() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 0, 2);
        c.on_sub_account_transferred(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            Ok(false),
        );
        assert_eq!(owned(&c, ALICE), vec![format!("alice.{TLA}")]);
        assert!(owned(&c, BOB).is_empty());
    }

    #[test]
    fn recovering_moves_the_name_between_owners() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(CAROL, 0, 2);
        c.on_sub_account_recovered(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            Ok(true),
        );
        assert!(owned(&c, ALICE).is_empty());
        assert_eq!(owned(&c, BOB), vec![format!("alice.{TLA}")]);
    }

    #[test]
    fn a_sale_moves_the_name_to_the_buyer() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_at(&mut c, "alice", 100);
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_sold(
            acc(TLA),
            "alice".to_string(),
            acc(BOB),
            U128(100),
            U128(100),
            Ok(true),
        );
        assert!(owned(&c, ALICE).is_empty());
        assert_eq!(owned(&c, BOB), vec![format!("alice.{TLA}")]);
    }

    #[test]
    fn a_custodial_sale_indexes_the_name_under_the_operator() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_at(&mut c, "alice", 100);
        ctx(ADMIN, 0, 2);
        c.add_payment_authority(acc(BOB)).unwrap();
        ctx(BOB, 0, 2);
        let _ = c
            .buy_sub_account_paid(acc(TLA), "alice".to_string(), acc(CAROL))
            .unwrap();
        c.on_sub_account_sold_paid(
            acc(TLA),
            "alice".to_string(),
            acc(CAROL),
            acc(BOB),
            U128(100),
            Ok(true),
        );
        assert!(owned(&c, ALICE).is_empty());
        assert_eq!(
            owned(&c, BOB),
            vec![format!("alice.{TLA}")],
            "a custodial buy leaves the operator holding the name"
        );
        assert!(owned(&c, CAROL).is_empty());
    }

    #[test]
    fn owner_paging_respects_offset_and_limit() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "bob");
        rent_alice_sub(&mut c, "carol");
        assert_eq!(c.list_sub_accounts_by_owner(acc(ALICE), 0, 2).len(), 2);
        assert_eq!(c.list_sub_accounts_by_owner(acc(ALICE), 2, 10).len(), 1);
        assert_eq!(c.list_sub_accounts_by_owner(acc(ALICE), 3, 10).len(), 0);
    }

    #[test]
    fn an_owner_with_no_names_pages_empty() {
        let c = deploy_with_open_tla();
        assert!(c.list_sub_accounts_by_owner(acc(BOB), 0, 10).is_empty());
    }

    #[test]
    fn activity_records_newest_first() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_at(&mut c, "alice", 10);
        let feed = c.list_recent_activity(0, 10, None);
        assert_eq!(feed[0].event, "sub_account_listed");
        assert_eq!(feed[0].account, format!("alice.{TLA}"));
        assert_eq!(feed[1].event, "sub_account_rented");
    }

    #[test]
    fn activity_paging_respects_offset_and_limit() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        list_at(&mut c, "alice", 10);
        assert_eq!(c.list_recent_activity(0, 1, None).len(), 1);
        assert_eq!(c.list_recent_activity(1, 10, None).len(), 1);
        assert_eq!(c.list_recent_activity(2, 10, None).len(), 0);
    }

    #[test]
    fn an_untouched_registry_has_an_empty_feed() {
        let c = deploy_with_open_tla();
        assert!(c.list_recent_activity(0, 10, None).is_empty());
    }

    #[test]
    fn a_scoped_feed_returns_a_full_page_for_that_account() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "bob");
        list_at(&mut c, "alice", 10);
        let alice = format!("alice.{TLA}");
        let scoped = c.list_recent_activity(0, 10, Some(alice.clone()));
        assert_eq!(scoped.len(), 2, "both of alice's events, none of bob's");
        assert!(scoped.iter().all(|e| e.account == alice));
        assert_eq!(
            c.list_recent_activity(0, 1, Some(alice)).len(),
            1,
            "the limit applies after filtering, so a page is never short"
        );
    }

    #[test]
    fn a_scoped_feed_for_an_unknown_account_is_empty() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        assert!(c
            .list_recent_activity(0, 10, Some("nobody.near".to_string()))
            .is_empty());
    }

    #[test]
    fn the_feed_wraps_and_keeps_the_newest_entries() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        for i in 0..ACTIVITY_CAPACITY + 5 {
            ctx(ALICE, 1, 2);
            c.list_sub_account(acc(TLA), "alice".to_string(), U128(u128::from(i) + 1))
                .unwrap();
            ctx(ALICE, 1, 3);
            c.unlist_sub_account(acc(TLA), "alice".to_string()).unwrap();
        }
        let feed = c.list_recent_activity(0, ACTIVITY_CAPACITY as u64 + 50, None);
        assert_eq!(
            feed.len(),
            ACTIVITY_CAPACITY as usize,
            "the buffer stays bounded"
        );
        assert_eq!(
            feed[0].event, "sub_account_unlisted",
            "the newest entry survives the wrap"
        );
        assert!(
            !feed.iter().any(|e| e.event == "sub_account_rented"),
            "the oldest entry is overwritten once the buffer wraps"
        );
    }

    #[test]
    fn names_are_indexed_under_their_namespace() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "bob");
        let page = names(c.list_sub_accounts_by_tla(acc(TLA), 0, 10));
        assert_eq!(page.len(), 2);
        assert!(page.contains(&format!("alice.{TLA}")));
    }

    #[test]
    fn a_transfer_leaves_the_namespace_index_alone() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 0, 2);
        c.on_sub_account_transferred(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            Ok(true),
        );
        assert_eq!(
            names(c.list_sub_accounts_by_tla(acc(TLA), 0, 10)),
            vec![format!("alice.{TLA}")],
            "a name never leaves its namespace, only its owner changes"
        );
    }

    #[test]
    fn reclaiming_drops_the_name_from_the_namespace_index() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE));
        assert!(c.list_sub_accounts_by_tla(acc(TLA), 0, 10).is_empty());
    }

    #[test]
    fn namespace_paging_respects_offset_and_limit() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "bob");
        assert_eq!(c.list_sub_accounts_by_tla(acc(TLA), 0, 1).len(), 1);
        assert_eq!(c.list_sub_accounts_by_tla(acc(TLA), 2, 10).len(), 0);
    }
}
