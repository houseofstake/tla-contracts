use crate::error::ContractError;
use crate::types::*;
use crate::{fees, TlaRegistry};
use hos_common::{MintOutcome, RotationCause};
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
    ctx(ADMIN, 1, 0);
    TlaRegistry::new(
        acc(ADMIN),
        acc(HOSEXT),
        U64(GRACE_NS),
        acc(TREASURY),
        acc(COUNCIL),
        None,
    )
}

fn deploy_priced() -> TlaRegistry {
    let mut c = deploy();
    ctx(ADMIN, 1, 0);
    c.admin_set_initial_rate(U128(NEAR_USD_MICRO)).unwrap();
    c
}

fn usd_to_near(usd_micro: u128) -> u128 {
    usd_micro * 1_000_000_000_000_000_000_000_000 / NEAR_USD_MICRO
}

fn deploy_with_open_tla() -> TlaRegistry {
    let mut c = deploy_priced();
    ctx(COUNCIL, 1, 0);
    c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
        .unwrap();
    ctx(ADMIN, 1, 0);
    c.activate_open_tla(acc(TLA)).unwrap();
    c
}

fn rent_total(c: &TlaRegistry, name: &str) -> u128 {
    let price = c.get_rent_price(acc(TLA), name.to_string()).unwrap();
    price.total_yocto.0
}

fn rent_usd_open(c: &TlaRegistry, name: &str) -> u128 {
    let label_len = u8::try_from(name.len()).unwrap_or(0);
    fees::sub_account_rent(label_len, &PremiumCategory::Standard, &c.get_fee_config())
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
        order_id: None,
    }
}

fn settled_for_order(
    name: &str,
    owner: &str,
    payer: &str,
    order_id: &str,
) -> crate::callbacks::MintSettlement {
    crate::callbacks::MintSettlement {
        order_id: Some(order_id.to_string()),
        ..settled(name, owner, payer, 0, 0)
    }
}

fn payout_of(c: &TlaRegistry, name: &str) -> AccountId {
    c.get_sub_account(acc(TLA), name.to_string())
        .unwrap()
        .payout_account
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

fn settle_transfer(c: &mut TlaRegistry, name: &str, from: &str, to: &str) {
    ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
    c.nft_on_rotation_resolved(
        crate::nft::NftRotation {
            tla_id: acc(TLA),
            name: name.to_string(),
            from: acc(from),
            to: acc(to),
            memo: None,
            cause: RotationCause::Transfer,
        },
        Ok(true),
    );
}

mod names {
    use super::*;
    use crate::error::NameInvalidReason;

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

    #[test]
    fn a_label_the_account_id_grammar_cannot_hold_is_refused_at_the_door() {
        let tla = acc(TLA);
        let widest = "a".repeat(64 - 1 - TLA.len());
        assert!(validate_mintable_name(&tla, &widest).is_ok());

        let one_over = "a".repeat(64 - TLA.len());
        assert!(validate_name(&one_over).is_ok());
        assert!(matches!(
            validate_mintable_name(&tla, &one_over),
            Err(ContractError::InvalidName {
                reason: NameInvalidReason::AccountIdTooLong
            })
        ));
    }

    #[test]
    fn a_single_character_label_is_refused_so_short_account_ids_stay_unmintable() {
        let tla = acc(TLA);
        assert!(validate_name("a").is_ok());
        assert!(matches!(
            validate_mintable_name(&tla, "a"),
            Err(ContractError::InvalidName {
                reason: NameInvalidReason::LabelTooShort
            })
        ));
        assert!(validate_mintable_name(&tla, "ab").is_ok());
    }

    #[test]
    fn a_quote_refuses_a_name_the_mint_could_never_accept() {
        let c = deploy_with_open_tla();
        let one_over = "a".repeat(64 - TLA.len());
        assert!(matches!(
            c.get_rent_price(acc(TLA), one_over),
            Err(ContractError::InvalidName {
                reason: NameInvalidReason::AccountIdTooLong
            })
        ));
        assert!(matches!(
            c.get_rent_price(acc(TLA), "-bad".to_string()),
            Err(ContractError::InvalidName {
                reason: NameInvalidReason::EdgeSeparator
            })
        ));
        assert!(matches!(
            c.get_rent_price(acc(TLA), "a".to_string()),
            Err(ContractError::InvalidName {
                reason: NameInvalidReason::LabelTooShort
            })
        ));
        assert!(c.get_rent_price(acc(TLA), "ab".to_string()).is_ok());
    }

    #[test]
    fn an_unmintable_name_costs_the_renter_nothing() {
        let mut c = deploy_with_open_tla();
        let one_over = "a".repeat(64 - TLA.len());
        let deposit = c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ALICE, deposit * 4, 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), one_over.clone(), None),
            Err(ContractError::InvalidName {
                reason: NameInvalidReason::AccountIdTooLong
            })
        ));
        assert!(
            c.get_sub_account(acc(TLA), one_over).is_none(),
            "rejecting at the door must leave no row behind to settle"
        );
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
        ctx(ADMIN, 1, 0);
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

    fn emitted_nft_transfers() -> Vec<near_sdk::serde_json::Value> {
        near_sdk::test_utils::get_logs()
            .iter()
            .filter(|l| l.starts_with("EVENT_JSON:"))
            .filter_map(|l| {
                near_sdk::serde_json::from_str::<near_sdk::serde_json::Value>(
                    l.trim_start_matches("EVENT_JSON:"),
                )
                .ok()
            })
            .filter(|j| j["standard"] == "nep171" && j["event"] == "nft_transfer")
            .collect()
    }

    #[test]
    fn a_recovery_tells_nep171_that_the_token_moved() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx_callback(near_sdk::PromiseResult::Successful(Vec::new()));
        c.on_sub_account_recovered(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            Ok(true),
        );
        let events = emitted_nft_transfers();
        assert_eq!(
            events.len(),
            1,
            "a wallet tracking nep171 must see recovery move the token"
        );
        assert_eq!(events[0]["data"][0]["old_owner_id"], ALICE);
        assert_eq!(events[0]["data"][0]["new_owner_id"], BOB);
    }

    #[test]
    fn a_direct_transfer_tells_nep171_that_the_token_moved() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx_callback(near_sdk::PromiseResult::Successful(Vec::new()));
        c.on_sub_account_transferred(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            RotationCause::Transfer,
            Ok(true),
        );
        assert_eq!(emitted_nft_transfers().len(), 1);
    }

    #[test]
    fn register_tla_emits_nep297_event() {
        let mut c = deploy();
        ctx(ADMIN, 1, 0);
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
        ctx(ALICE, 1, 0);
        assert!(matches!(
            c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn admin_controls_payment_authorities() {
        let mut c = deploy();
        ctx(ADMIN, 1, 0);
        c.add_payment_authority(acc(BOB)).unwrap();
        assert_eq!(c.get_payment_authorities(), vec![acc(BOB)]);
        c.remove_payment_authority(acc(BOB)).unwrap();
        assert!(c.get_payment_authorities().is_empty());
    }

    #[test]
    fn admin_controls_recovery_authorities() {
        let mut c = deploy();
        ctx(ADMIN, 1, 0);
        c.add_recovery_authority(acc(BOB)).unwrap();
        assert_eq!(c.get_recovery_authorities(), vec![acc(BOB)]);
        c.remove_recovery_authority(acc(BOB)).unwrap();
        assert!(c.get_recovery_authorities().is_empty());
    }

    #[test]
    fn only_the_council_grants_the_recovery_role() {
        let mut c = deploy();
        ctx(ALICE, 1, 0);
        assert!(matches!(
            c.add_recovery_authority(acc(BOB)),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn duplicate_registration_rejected() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 1, 0);
        assert!(matches!(
            c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None),
            Err(ContractError::TlaAlreadyRegistered)
        ));
    }

    #[test]
    fn business_tla_requires_licensee() {
        let mut c = deploy();
        ctx(ADMIN, 1, 0);
        assert!(matches!(
            c.register_tla(acc(TLA), TlaType::Business, PremiumCategory::Standard, None),
            Err(ContractError::BusinessTlaRequiresLicensee)
        ));
    }

    #[test]
    fn open_tla_rejects_business_activation_endpoint() {
        let mut c = deploy_priced();
        ctx(ADMIN, 1, 0);
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
        ctx(ADMIN, 1, 1);
        c.suspend_tla(acc(TLA)).unwrap();
        ctx(ALICE, rent_total(&c, "alice"), 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "alice".to_string(), None),
            Err(ContractError::TlaNotAcceptingRentals)
        ));
    }

    #[test]
    fn a_lapsed_suspension_frees_the_tla_without_anyone_unsuspending_it() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 1, 1);
        c.suspend_tla(acc(TLA)).unwrap();
        let after = hos_common::MAX_AUTHORITY_HOLD_NS + 1;

        ctx(ADMIN, 1, after);
        assert!(
            matches!(
                c.get_tla(acc(TLA)).unwrap().lifecycle,
                LifecycleStatus::Active
            ),
            "the hold is bounded on purpose, so a lapsed suspension must stop reporting as one"
        );

        assert!(
            c.tlas
                .get(&acc(TLA))
                .unwrap()
                .accepting_rentals(c.suspension_expiry(&acc(TLA))),
            "renting and selling read the same lapse, so they must not disagree about it"
        );
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
        ctx(ADMIN, 1, 1);
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
        ctx(ADMIN, 1, 1);
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
            "the sponsor paid, so the sponsor is the one made whole"
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
        ctx(ADMIN, 1, 1);
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
            c.rent_sub_account_paid(
                acc(TLA),
                "alice".to_string(),
                acc(ALICE),
                acc(ALICE),
                "ord-unauthorised".to_string()
            ),
            Err(ContractError::OnlyPaymentAuthority)
        ));

        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(BOB)).unwrap();
        c.bind_payment_authority_tla(acc(BOB), acc(TLA), U64(50))
            .unwrap();
        ctx(BOB, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "alice".to_string(),
                acc(ALICE),
                acc(ALICE),
                "ord-paid-alice".to_string(),
            )
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
    fn a_mint_that_reports_no_outcome_refunds_in_full_and_parks_the_name() {
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
        assert_eq!(
            c.get_pending_refund(acc(ALICE)).0,
            total,
            "the registry cannot prove the account was created, so the renter is never the one out of pocket"
        );
        assert!(
            !c.is_name_available(acc(TLA), "alice".to_string()),
            "an account may exist behind this name, so it must not go back on the mint path"
        );
        assert!(
            c.is_name_re_rentable(acc(TLA), "alice".to_string()),
            "parking keeps the name reachable instead of stranding it forever"
        );
        assert_eq!(c.get_stats().sub_account_count, 0);
    }

    #[test]
    fn a_reported_mint_failure_frees_the_name_instead_of_parking_it() {
        let mut c = deploy_with_open_tla();
        let rent_near = rent_near_open(&c, "alice");
        let total = rent_total(&c, "alice");
        ctx(ALICE, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), None)
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_sub_account_created(
            settled("alice", ALICE, ALICE, rent_near, total),
            Ok(MintOutcome::CreationFailed),
        );

        assert_eq!(
            c.get_pending_refund(acc(ALICE)).0,
            total,
            "the registrar reported the failure, so it returned the funding and the payer is whole"
        );
        assert!(
            c.is_name_available(acc(TLA), "alice".to_string()),
            "a reported failure proves no account exists, so the name goes straight back on sale"
        );
        assert!(!c.is_name_re_rentable(acc(TLA), "alice".to_string()));
    }

    #[test]
    fn an_admin_releases_a_park_whose_account_never_existed() {
        let mut c = deploy_with_open_tla();
        let rent_near = rent_near_open(&c, "alice");
        let total = rent_total(&c, "alice");
        ctx(ALICE, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), None)
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_sub_account_created(
            settled("alice", ALICE, ALICE, rent_near, total),
            Err(PromiseError::Failed),
        );
        assert!(c.is_name_re_rentable(acc(TLA), "alice".to_string()));

        ctx(BOB, 1, 2);
        assert!(matches!(
            c.admin_release_park(acc(TLA), "alice".to_string()),
            Err(ContractError::OnlyAdmin)
        ));

        ctx(ADMIN, 1, 2);
        c.admin_release_park(acc(TLA), "alice".to_string()).unwrap();
        assert!(c.is_name_available(acc(TLA), "alice".to_string()));
        assert!(!c.is_name_re_rentable(acc(TLA), "alice".to_string()));

        assert!(
            matches!(
                c.admin_release_park(acc(TLA), "alice".to_string()),
                Err(ContractError::SubAccountNotParked)
            ),
            "releasing is one-shot, so a second call cannot quietly do nothing"
        );
    }

    #[test]
    fn an_admin_cannot_release_the_park_under_a_live_lease() {
        let mut c = deploy_with_open_tla();
        let rent_near = rent_near_open(&c, "alice");
        let total = rent_total(&c, "alice");
        ctx(ALICE, total, 1);
        let _ = c
            .rent_sub_account(acc(TLA), "alice".to_string(), None)
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_sub_account_created(
            settled("alice", ALICE, ALICE, rent_near, total),
            Err(PromiseError::Failed),
        );

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
        let _ = c.on_sub_account_re_rented(settled("alice", BOB, BOB, rent, rent), Ok(true));

        ctx(ADMIN, 1, 4);
        assert!(
            matches!(
                c.admin_release_park(acc(TLA), "alice".to_string()),
                Err(ContractError::SubAccountNameTaken)
            ),
            "the hatch must never strip a park out from under a name someone now holds"
        );
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
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_renewed(
            acc(TLA),
            "alice".to_string(),
            U64(before + ONE_YEAR_NS),
            acc(ALICE),
            U128(rent),
        );
        let after = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        assert_eq!(after, before + ONE_YEAR_NS);
    }

    #[test]
    fn a_renewal_the_wallet_refused_charges_nothing_and_extends_nothing() {
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
        let revenue_before = c.get_stats().total_revenue_yocto.0;

        ctx(ALICE, rent, 2);
        let _ = c.renew_sub_account(acc(TLA), "alice".to_string()).unwrap();
        let owed_before = c.get_pending_refund(acc(ALICE)).0;
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_sub_account_renewed(
            acc(TLA),
            "alice".to_string(),
            U64(before + ONE_YEAR_NS),
            acc(ALICE),
            U128(rent),
        );

        let after = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        assert_eq!(
            after, before,
            "the wallet refused the lease, so the registry must not report it renewed"
        );
        assert_eq!(
            c.get_stats().total_revenue_yocto.0,
            revenue_before,
            "rent must not be booked for a renewal the wallet never took"
        );
        assert_eq!(
            c.get_pending_refund(acc(ALICE)).0 - owed_before,
            rent,
            "the payer is owed the rent back"
        );
    }

    #[test]
    fn a_wallet_that_refused_a_lease_update_says_so_out_loud() {
        let mut c = deploy_with_open_tla();
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_lease_synced(format!("alice.{TLA}"), "retract".to_string());
        let logs = near_sdk::test_utils::get_logs();
        assert!(
            logs.iter().any(|l| l.contains("lease_sync_failed")),
            "the registry and the wallet each hold a copy of the term, so a refused update has \
             to be visible rather than leaving the two silently disagreeing, got {logs:?}"
        );
    }

    #[test]
    fn a_wallet_that_took_the_lease_update_stays_quiet() {
        let mut c = deploy_with_open_tla();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_lease_synced(format!("alice.{TLA}"), "retract".to_string());
        assert!(
            !near_sdk::test_utils::get_logs()
                .iter()
                .any(|l| l.contains("lease_sync_failed")),
            "a successful push must not raise the alarm"
        );
    }

    #[test]
    fn a_fresh_registry_reports_every_missing_deploy_step() {
        let c = deploy();
        let state = c.deployment_readiness();
        assert!(!state.ready);
        assert!(!state.recovery_wired, "no recovery authority yet");
        assert!(!state.venue_set, "no venue yet");
        assert!(
            !state.ft_allowlist_set,
            "an empty allowlist skips the balance gate, so a name changes hands \
             carrying whatever tokens the last holder left in it"
        );
        assert!(
            !state.metadata_set,
            "name still derives from the account id"
        );
        assert!(state.wiring_sane, "constructor already refuses self-wiring");
    }

    #[test]
    fn readiness_flips_only_once_every_step_is_done() {
        let mut c = deploy_with_open_tla();
        assert!(!c.deployment_readiness().ready);
        ctx(COUNCIL, 1, 1);
        c.add_recovery_authority(acc(BOB)).unwrap();
        assert!(!c.deployment_readiness().ready, "venue still missing");
        ctx(COUNCIL, 1, 1);
        c.add_venue(acc("venue.testnet")).unwrap();
        assert!(
            !c.deployment_readiness().ready,
            "ft allowlist still missing"
        );
        ctx(COUNCIL, 1, 1);
        c.add_ft_allowlist(acc("usdc.testnet")).unwrap();
        assert!(!c.deployment_readiness().ready, "metadata still missing");
        ctx(ADMIN, 1, 1);
        c.admin_set_nft_metadata("Names".to_string(), "NAME".to_string(), None, None, None)
            .unwrap();
        let done = c.deployment_readiness();
        assert!(
            done.ready,
            "every step done but readiness still false: {done:?}"
        );
    }

    #[test]
    fn only_the_council_may_name_a_venue() {
        let mut c = deploy_with_open_tla();
        ctx(BOB, 1, 1);
        assert!(matches!(
            c.add_venue(acc("venue.testnet")),
            Err(ContractError::OnlyCouncil)
        ));
        ctx(COUNCIL, 1, 1);
        c.add_venue(acc("venue.testnet")).unwrap();
        assert_eq!(c.get_venues(), vec![acc("venue.testnet")]);
        ctx(COUNCIL, 1, 1);
        c.remove_venue(acc("venue.testnet")).unwrap();
        assert!(c.get_venues().is_empty());
    }

    #[test]
    fn the_registry_cannot_name_itself_a_venue() {
        let mut c = deploy_with_open_tla();
        ctx(COUNCIL, 1, 1);
        assert!(matches!(
            c.add_venue(near_sdk::env::current_account_id()),
            Err(ContractError::VenueIsRegistry)
        ));
    }

    #[test]
    fn the_venue_list_is_bounded() {
        let mut c = deploy_with_open_tla();
        for i in 0..8 {
            ctx(COUNCIL, 1, 1);
            c.add_venue(acc(&format!("venue{i}.testnet"))).unwrap();
        }
        ctx(COUNCIL, 1, 1);
        assert!(matches!(
            c.add_venue(acc("one-too-many.testnet")),
            Err(ContractError::AllowlistFull)
        ));
    }

    #[test]
    fn a_seal_that_did_not_remove_the_key_is_not_reported_as_sealed() {
        let mut c = deploy_with_open_tla();
        ctx_callback(near_sdk::PromiseResult::Failed);
        assert!(!c.after_seal("ed25519:key".to_string(), acc(COUNCIL)));
        let logs = near_sdk::test_utils::get_logs();
        assert!(logs.iter().any(|l| l.contains("seal_failed")));
        assert!(
            !logs.iter().any(|l| l.contains(r#""event":"sealed""#)),
            "the launch gate reads this log, so a key that survived must never read as sealed"
        );
    }

    #[test]
    fn the_admin_set_is_bounded_so_its_view_cannot_outgrow_one_call() {
        let mut c = deploy_with_open_tla();
        while c.get_admins().len() < 32 {
            let next = format!("admin{}.testnet", c.get_admins().len());
            ctx(COUNCIL, 1, 1);
            c.add_admin(acc(&next)).unwrap();
        }
        ctx(COUNCIL, 1, 1);
        assert!(matches!(
            c.add_admin(acc("one-too-many.testnet")),
            Err(ContractError::AuthoritySetFull)
        ));
        ctx(COUNCIL, 1, 1);
        c.remove_admin(acc("admin31.testnet")).unwrap();
        ctx(COUNCIL, 1, 1);
        assert!(
            c.add_admin(acc("one-too-many.testnet")).is_ok(),
            "the cap bounds the set, it does not close the seat permanently"
        );
    }

    #[test]
    fn the_payment_authority_set_is_bounded() {
        let mut c = deploy_with_open_tla();
        for i in 0..32 {
            ctx(COUNCIL, 1, 1);
            c.add_payment_authority(acc(&format!("pay{i}.testnet")))
                .unwrap();
        }
        ctx(COUNCIL, 1, 1);
        assert!(matches!(
            c.add_payment_authority(acc("one-too-many.testnet")),
            Err(ContractError::AuthoritySetFull)
        ));
    }

    #[test]
    fn the_recovery_authority_set_is_bounded() {
        let mut c = deploy_with_open_tla();
        while c.get_recovery_authorities().len() < 32 {
            let next = format!("rec{}.testnet", c.get_recovery_authorities().len());
            ctx(COUNCIL, 1, 1);
            c.add_recovery_authority(acc(&next)).unwrap();
        }
        ctx(COUNCIL, 1, 1);
        assert!(matches!(
            c.add_recovery_authority(acc("one-too-many.testnet")),
            Err(ContractError::AuthoritySetFull)
        ));
    }

    #[test]
    fn a_stranger_can_pay_to_renew_a_name_they_do_not_own() {
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
        ctx(BOB, rent, 2);
        let _ = c.renew_sub_account(acc(TLA), "alice".to_string()).unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_renewed(
            acc(TLA),
            "alice".to_string(),
            U64(before + ONE_YEAR_NS),
            acc(BOB),
            U128(rent),
        );
        let sub = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(sub.expires_at.0, before + ONE_YEAR_NS);
        assert_eq!(sub.owner, acc(ALICE));
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
        assert!(c
            .set_payout_account(acc(TLA), "alice".to_string(), acc(BOB))
            .is_ok());
        assert_eq!(
            payout_of(&c, "alice"),
            acc(ALICE),
            "the record must not move before the wallet has taken it"
        );
        ctx_callback(near_sdk::PromiseResult::Successful(Vec::new()));
        c.on_payout_set(acc(TLA), "alice".to_string(), acc(BOB), acc(ALICE));
        assert_eq!(payout_of(&c, "alice"), acc(BOB));
    }

    #[test]
    fn a_payout_the_wallet_refused_leaves_the_record_alone() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        assert!(c
            .set_payout_account(acc(TLA), "alice".to_string(), acc(BOB))
            .is_ok());
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_payout_set(acc(TLA), "alice".to_string(), acc(BOB), acc(ALICE));
        assert_eq!(
            payout_of(&c, "alice"),
            acc(ALICE),
            "otherwise the registry says one thing and the sweep does another"
        );
    }

    #[test]
    fn a_payout_change_does_not_land_on_a_name_that_changed_hands() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        assert!(c
            .set_payout_account(acc(TLA), "alice".to_string(), acc(CAROL))
            .is_ok());
        let key = sub_account_key(&acc(TLA), "alice");
        assert!(c.sub_account_reassign(&key, &acc(BOB), &acc(BOB)));
        ctx_callback(near_sdk::PromiseResult::Successful(Vec::new()));
        c.on_payout_set(acc(TLA), "alice".to_string(), acc(CAROL), acc(ALICE));
        assert_eq!(
            payout_of(&c, "alice"),
            acc(BOB),
            "the seller must not redirect the payout of a name the buyer now holds"
        );
    }
}

mod marketplace {
    use super::*;

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
    fn a_resolve_does_not_claw_a_name_back_from_a_third_party() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = format!("alice.{TLA}");
        settle_transfer(&mut c, "alice", ALICE, BOB);
        settle_transfer(&mut c, "alice", BOB, CAROL);
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        let outcome = c.nft_resolve_transfer(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            Ok(true),
        );
        assert!(
            matches!(outcome, near_sdk::PromiseOrValue::Value(true)),
            "a receiver that moved the name on must not be able to revert it away from its new holder"
        );
        assert_eq!(
            c.nft_token(key).unwrap().owner_id,
            acc(CAROL),
            "the third party keeps the name"
        );
    }

    #[test]
    fn a_give_back_points_the_payout_at_the_owner_it_returns_to() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        settle_transfer(&mut c, "alice", ALICE, BOB);
        assert_eq!(
            payout_of(&c, "alice"),
            acc(BOB),
            "a plain transfer repoints the payout at the receiver"
        );

        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        let kept = c.nft_on_return_resolved(
            acc(TLA),
            "alice".to_string(),
            acc(BOB),
            acc(ALICE),
            Ok(true),
        );
        assert!(!kept, "the receiver handed the name back");
        assert_eq!(
            c.nft_token(format!("alice.{TLA}")).unwrap().owner_id,
            acc(ALICE)
        );
        assert_eq!(
            payout_of(&c, "alice"),
            acc(ALICE),
            "rent and sweeps must follow the name home, never stay aimed at the receiver"
        );
    }

    #[test]
    fn a_co_owner_cannot_sell_a_name_they_do_not_own() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 1, 2);
        assert!(matches!(
            c.nft_transfer(acc(BOB), format!("alice.{TLA}"), None, None),
            Err(ContractError::OnlyOwner)
        ));
    }

    #[test]
    fn an_unknown_token_id_is_refused_before_any_rotation() {
        let mut c = deploy_with_open_tla();
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.nft_transfer(acc(BOB), format!("ghost.{TLA}"), None, None),
            Err(ContractError::TokenNotFound)
        ));
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
    fn transfer_rejects_another_registered_name_as_recipient() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "bob");
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.transfer_sub_account(acc(TLA), "alice".to_string(), acc(&format!("bob.{TLA}"))),
            Err(ContractError::TransferToRegisteredName)
        ));
    }

    #[test]
    fn transfer_still_allows_an_ordinary_account_as_recipient() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        assert!(c
            .transfer_sub_account(acc(TLA), "alice".to_string(), acc(BOB))
            .is_ok());
    }

    #[test]
    fn depositing_into_a_venue_is_not_caught_by_the_registered_name_guard() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "venue");
        let venue = format!("venue.{TLA}");
        ctx(COUNCIL, 1, 1);
        c.add_venue(acc(&venue)).unwrap();
        ctx(ALICE, 1, 2);
        assert!(c
            .transfer_sub_account(acc(TLA), "alice".to_string(), acc(&venue))
            .is_ok());
    }

    #[test]
    fn withdrawing_from_a_venue_is_never_refused_by_the_registered_name_guard() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "holder");
        let venue = acc("venue.testnet");
        ctx(COUNCIL, 1, 1);
        c.add_venue(venue.clone()).unwrap();
        let key = format!("alice.{TLA}");
        if let Some(sub) = c.sub_accounts.get_mut(&key) {
            sub.owner = venue.clone();
        }
        ctx(venue.as_str(), 1, 2);
        assert!(c
            .transfer_sub_account(acc(TLA), "alice".to_string(), acc(&format!("holder.{TLA}")))
            .is_ok());
    }

    #[test]
    fn recovery_rejects_another_registered_name_as_destination() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "bob");
        ctx(COUNCIL, 1, 1);
        c.add_recovery_authority(acc(BOB)).unwrap();
        ctx(BOB, 1, 2);
        assert!(matches!(
            c.recover_sub_account(
                acc(TLA),
                "alice".to_string(),
                acc(&format!("bob.{TLA}")),
                acc(ALICE)
            ),
            Err(ContractError::TransferToRegisteredName)
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
            RotationCause::Transfer,
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
            RotationCause::Transfer,
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
        ctx(ADMIN, 1, 0);
        c.add_recovery_authority(acc(CAROL)).unwrap();
        c
    }

    #[test]
    fn a_stranger_cannot_recover_a_name() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 1, 2);
        assert!(matches!(
            c.recover_sub_account(acc(TLA), "alice".to_string(), acc(BOB), acc(ALICE)),
            Err(ContractError::OnlyRecoveryAuthority)
        ));
    }

    #[test]
    fn the_current_owner_cannot_recover_their_own_name() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.recover_sub_account(acc(TLA), "alice".to_string(), acc(BOB), acc(ALICE)),
            Err(ContractError::OnlyRecoveryAuthority)
        ));
    }

    #[test]
    fn recovery_hands_the_name_to_a_new_owner_account() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(CAROL, 1, 2);
        assert!(c
            .recover_sub_account(acc(TLA), "alice".to_string(), acc(BOB), acc(ALICE))
            .is_ok());
    }

    #[test]
    fn recovery_refuses_a_name_that_moved_since_the_request() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        let key = sub_account_key(&acc(TLA), "alice");
        assert!(c.sub_account_reassign(&key, &acc(BOB), &acc(BOB)));
        ctx(CAROL, 1, 2);
        assert!(
            matches!(
                c.recover_sub_account(
                    acc(TLA),
                    "alice".to_string(),
                    acc("dave.testnet"),
                    acc(ALICE)
                ),
                Err(ContractError::OwnerMoved)
            ),
            "a name sold or deposited since the request must not be recovered away"
        );
    }

    #[test]
    fn recovery_requires_one_yocto() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(CAROL, 0, 2);
        assert!(matches!(
            c.recover_sub_account(acc(TLA), "alice".to_string(), acc(BOB), acc(ALICE)),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn recovery_rejects_a_no_op_owner_change() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
        ctx(CAROL, 1, 2);
        assert!(matches!(
            c.recover_sub_account(acc(TLA), "alice".to_string(), acc(ALICE), acc(ALICE)),
            Err(ContractError::SameOwner)
        ));
    }

    #[test]
    fn recovery_refuses_an_unknown_name() {
        let mut c = deploy_with_recovery();
        ctx(CAROL, 1, 2);
        assert!(matches!(
            c.recover_sub_account(acc(TLA), "nobody".to_string(), acc(BOB), acc(ALICE)),
            Err(ContractError::SubAccountNotFound)
        ));
    }

    #[test]
    fn recovery_callback_moves_owner_and_payout() {
        let mut c = deploy_with_recovery();
        rent_alice_sub(&mut c, "alice");
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
        assert_eq!(
            c.nft_token(key).unwrap().owner_id,
            acc(BOB),
            "a recovered name must read as owned by the recovered account"
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
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE), Ok(true));
        assert!(c.is_name_re_rentable(acc(TLA), "alice".to_string()));
        assert!(!c.is_name_available(acc(TLA), "alice".to_string()));
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
        let _ = c.on_sub_account_re_rented(settled("alice", BOB, BOB, rent, rent), Ok(true));
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
    fn the_paid_lane_recycles_a_parked_name() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE), Ok(true));
        assert!(c.is_name_re_rentable(acc(TLA), "alice".to_string()));

        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(BOB)).unwrap();
        c.bind_payment_authority_tla(acc(BOB), acc(TLA), U64(50))
            .unwrap();
        ctx(BOB, 0, 2);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "alice".to_string(),
                acc(CAROL),
                acc(CAROL),
                "ord-re-rent".to_string(),
            )
            .expect("a parked name is re-rentable through the paid lane");
        assert_eq!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .owner,
            acc(CAROL)
        );
    }

    #[test]
    fn a_failed_park_does_not_reclaim_the_name() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE), Ok(false));
        assert!(!c.is_name_re_rentable(acc(TLA), "alice".to_string()));
        assert_eq!(c.get_stats().sub_account_count, 1);
        assert_eq!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .owner,
            acc(ALICE)
        );
    }

    #[test]
    fn a_park_whose_receipt_failed_does_not_reclaim_the_name() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_reclaim_finalized(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            Err(near_sdk::PromiseError::Failed),
        );
        assert!(!c.is_name_re_rentable(acc(TLA), "alice".to_string()));
        assert_eq!(c.get_stats().sub_account_count, 1);
    }

    #[test]
    fn a_failed_owner_swap_does_not_complete_a_re_rent() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE), Ok(true));
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
        let _ = c.on_sub_account_re_rented(settled("alice", BOB, BOB, rent, rent), Ok(false));
        assert!(c.is_name_re_rentable(acc(TLA), "alice".to_string()));
        assert!(c.get_sub_account(acc(TLA), "alice".to_string()).is_none());
    }

    #[test]
    fn double_reclaim_does_not_double_decrement() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        assert_eq!(c.get_stats().sub_account_count, 1);
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE), Ok(true));
        assert_eq!(c.get_stats().sub_account_count, 0);
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE), Ok(true));
        assert_eq!(c.get_stats().sub_account_count, 0);
    }

    #[test]
    #[should_panic(expected = "grace period too short")]
    fn new_rejects_short_grace_period() {
        ctx(ADMIN, 1, 0);
        let _ = TlaRegistry::new(
            acc(ADMIN),
            acc(HOSEXT),
            U64(0),
            acc(TREASURY),
            acc(COUNCIL),
            None,
        );
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
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE), Ok(true));
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
        ctx(ADMIN, 1, 2);
        c.admin_clear_reclaim_pending(acc(TLA), "alice".to_string())
            .unwrap();
        assert!(!c.is_reclaim_in_progress(acc(TLA), "alice".to_string()));
    }

    #[test]
    fn a_pending_reclaim_blocks_every_transfer_path() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = format!("alice.{TLA}");
        c.reclaim_pending.insert(key.clone(), true);
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.nft_transfer(acc(BOB), key.clone(), None, None),
            Err(ContractError::ReclaimInProgress)
        ));
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.nft_transfer_call(acc(BOB), key, None, None, String::new()),
            Err(ContractError::ReclaimInProgress)
        ));
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.transfer_sub_account(acc(TLA), "alice".to_string(), acc(BOB)),
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
    fn a_refund_cannot_be_claimed_twice() {
        let mut c = deploy();
        c.add_pending_refund(&acc(ALICE), 500);
        assert_eq!(c.get_pending_refund(acc(ALICE)).0, 500);

        ctx(ALICE, 0, 1);
        assert!(c.claim_refund().is_ok());
        assert_eq!(
            c.get_pending_refund(acc(ALICE)).0,
            0,
            "the entry has to clear before the transfer, or a second call in the same block \
             pays the same refund again"
        );
        assert_eq!(c.total_pending_refunds, 0);

        ctx(ALICE, 0, 1);
        assert!(matches!(
            c.claim_refund(),
            Err(ContractError::NoPendingRefund)
        ));
    }

    #[test]
    fn a_failed_refund_transfer_restores_the_entry() {
        let mut c = deploy();
        c.add_pending_refund(&acc(ALICE), 500);
        ctx(ALICE, 0, 1);
        assert!(c.claim_refund().is_ok());
        assert_eq!(c.get_pending_refund(acc(ALICE)).0, 0);

        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_claim_refund_settled(acc(ALICE), U128(500));
        assert_eq!(
            c.get_pending_refund(acc(ALICE)).0,
            500,
            "a refund whose transfer failed must remain claimable"
        );
    }

    #[test]
    fn withdraw_capped_by_revenue() {
        let mut c = deploy();
        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.withdraw(U128(1)),
            Err(ContractError::InsufficientRevenue)
        ));
    }

    #[test]
    fn pause_blocks_rent() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 1, 1);
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
        ctx(ADMIN, 1, 1);
        c.add_ft_allowlist(acc("token.testnet")).unwrap();
        assert_eq!(c.get_ft_allowlist(), vec![acc("token.testnet")]);
        c.remove_ft_allowlist(acc("token.testnet")).unwrap();
        assert!(c.get_ft_allowlist().is_empty());
    }

    #[test]
    fn relisting_a_token_never_exhausts_the_sweep_ceiling() {
        let mut c = deploy();
        ctx(ADMIN, 1, 1);
        let token = acc("usdc.testnet");
        for _ in 0..200 {
            ctx(ADMIN, 1, 1);
            c.add_ft_allowlist(token.clone()).unwrap();
            c.remove_ft_allowlist(token.clone()).unwrap();
        }
        ctx(ADMIN, 1, 1);
        c.add_ft_allowlist(token.clone()).unwrap();
        assert_eq!(
            c.get_ft_allowlist(),
            vec![token],
            "a token already known to the sweeper must be re-listable forever"
        );
        assert!(
            c.add_ft_allowlist(acc("other.testnet")).is_ok(),
            "and churning one token must not consume another's capacity"
        );
    }

    #[test]
    fn fee_config_guards() {
        let mut c = deploy();
        ctx(ADMIN, 1, 1);
        let mut config = c.get_fee_config();
        config.max_rate_move_bps = 10_001;
        assert!(matches!(
            c.update_fee_config(config),
            Err(ContractError::InvalidRateBounds)
        ));
    }

    #[test]
    fn rent_tiers_must_not_price_a_short_name_below_a_long_one() {
        let mut c = deploy();
        ctx(ADMIN, 1, 1);
        let mut config = c.get_fee_config();
        config.rent_tier_5_usd_micro = U128(1);
        config.rent_tier_12plus_usd_micro = U128(1_000);
        assert!(matches!(
            c.update_fee_config(config),
            Err(ContractError::RentTiersNotDescending)
        ));
    }
}

mod partner_terms {
    use super::*;

    fn registered_business(c: &mut TlaRegistry) {
        ctx(ADMIN, 1, 0);
        c.register_tla(
            acc(TLA),
            TlaType::Business,
            PremiumCategory::Standard,
            Some(acc(ALICE)),
        )
        .unwrap();
    }

    fn terms(allocation: Option<u128>, rent: Option<u128>, sub: Option<u128>) -> TlaTerms {
        TlaTerms {
            allocation_fee_usd_micro: allocation.map(U128),
            tla_rent_usd_micro: rent.map(U128),
            sub_fee_usd_micro: sub.map(U128),
        }
    }

    #[test]
    fn a_partner_can_be_given_a_namespace_for_nothing() {
        let mut c = deploy_priced();
        registered_business(&mut c);
        ctx(COUNCIL, 1, 0);
        c.set_tla_terms(acc(TLA), terms(Some(0), Some(0), Some(0)))
            .unwrap();

        ctx(ALICE, 0, 0);
        assert!(
            c.activate_tla(acc(TLA)).is_ok(),
            "a negotiated free namespace must not need the standard allocation fee"
        );
    }

    #[test]
    fn a_negotiated_rate_is_charged_instead_of_the_schedule() {
        let mut c = deploy_priced();
        registered_business(&mut c);
        ctx(COUNCIL, 1, 0);
        c.set_tla_terms(
            acc(TLA),
            terms(
                Some(7 * crate::pricing::USD_MICRO_PER_DOLLAR),
                Some(0),
                None,
            ),
        )
        .unwrap();

        let owed = usd_to_near(7 * crate::pricing::USD_MICRO_PER_DOLLAR);
        ctx(ALICE, owed.saturating_sub(1), 0);
        assert!(
            matches!(
                c.activate_tla(acc(TLA)),
                Err(ContractError::InsufficientPayment)
            ),
            "the deal price is still a price, not a waiver"
        );
        ctx(ALICE, owed, 0);
        assert!(c.activate_tla(acc(TLA)).is_ok());
    }

    #[test]
    fn a_tla_on_standard_terms_pays_the_schedule() {
        let mut c = deploy_priced();
        registered_business(&mut c);
        let scheduled = usd_to_near(
            c.get_fee_config().tla_allocation_fee_usd_micro.0
                + fees::base_rent(TLA.len() as u8, &c.get_fee_config()),
        );
        ctx(ALICE, scheduled.saturating_sub(1), 0);
        assert!(matches!(
            c.activate_tla(acc(TLA)),
            Err(ContractError::InsufficientPayment)
        ));
        ctx(ALICE, scheduled, 0);
        assert!(c.activate_tla(acc(TLA)).is_ok());
    }

    #[test]
    fn a_per_name_rate_overrides_the_global_business_fee() {
        let mut c = deploy_priced();
        registered_business(&mut c);
        ctx(COUNCIL, 1, 0);
        c.set_tla_terms(acc(TLA), terms(Some(0), Some(0), None))
            .unwrap();
        ctx(ALICE, 0, 0);
        c.activate_tla(acc(TLA)).unwrap();
        let scheduled = c
            .get_rent_price(acc(TLA), "staff".to_string())
            .unwrap()
            .rent_yocto
            .0;
        assert!(scheduled > 0, "the schedule must charge something to start");

        ctx(COUNCIL, 1, 0);
        c.set_tla_terms(acc(TLA), terms(Some(0), Some(0), Some(0)))
            .unwrap();
        assert_eq!(
            c.get_rent_price(acc(TLA), "staff".to_string())
                .unwrap()
                .rent_yocto
                .0,
            0,
            "the quote must follow the agreement, not the fee schedule"
        );
    }

    #[test]
    fn clearing_the_terms_returns_the_tla_to_the_schedule() {
        let mut c = deploy_priced();
        registered_business(&mut c);
        ctx(COUNCIL, 1, 0);
        c.set_tla_terms(acc(TLA), terms(Some(0), Some(0), Some(0)))
            .unwrap();
        ctx(COUNCIL, 1, 0);
        c.set_tla_terms(acc(TLA), terms(None, None, None)).unwrap();

        assert!(
            !c.tla_terms.contains_key(&acc(TLA)),
            "a cleared agreement must leave no override row behind to pay storage on"
        );
        assert!(c.get_tla_terms(acc(TLA)).is_standard());
        ctx(ALICE, 0, 0);
        assert!(matches!(
            c.activate_tla(acc(TLA)),
            Err(ContractError::InsufficientPayment)
        ));
    }

    #[test]
    fn only_the_council_may_write_a_partner_agreement() {
        let mut c = deploy_priced();
        registered_business(&mut c);
        ctx(ALICE, 1, 0);
        assert!(
            matches!(
                c.set_tla_terms(acc(TLA), terms(Some(0), Some(0), Some(0))),
                Err(ContractError::OnlyCouncil)
            ),
            "a licensee must not be able to price their own agreement"
        );
    }

    #[test]
    fn a_per_name_rate_is_refused_on_an_open_tla_that_would_ignore_it() {
        let mut c = deploy_with_open_tla();
        ctx(COUNCIL, 1, 0);
        assert!(
            matches!(
                c.set_tla_terms(acc(TLA), terms(None, None, Some(0))),
                Err(ContractError::NotBusinessTla)
            ),
            "an open TLA prices names by length tier, so a per-name rate set here \
             would read as applied and never be charged"
        );
        ctx(COUNCIL, 1, 0);
        assert!(
            c.set_tla_terms(acc(TLA), terms(Some(0), Some(0), None))
                .is_ok(),
            "the allocation and rent halves still apply to an open TLA"
        );
    }

    #[test]
    fn terms_cannot_be_written_for_a_tla_that_does_not_exist() {
        let mut c = deploy_priced();
        ctx(COUNCIL, 1, 0);
        assert!(matches!(
            c.set_tla_terms(acc("nosuch.testnet"), terms(Some(0), None, None)),
            Err(ContractError::TlaNotFound)
        ));
    }

    #[test]
    fn a_partner_rate_is_still_bound_by_the_fee_ceiling() {
        let mut c = deploy_priced();
        registered_business(&mut c);
        ctx(COUNCIL, 1, 0);
        assert!(matches!(
            c.set_tla_terms(
                acc(TLA),
                terms(Some(crate::pricing::MAX_USD_MICRO + 1), None, None)
            ),
            Err(ContractError::FeeExceedsCap)
        ));
    }

    #[test]
    fn a_renewal_follows_the_agreement_the_activation_used() {
        let mut c = deploy_priced();
        registered_business(&mut c);
        ctx(COUNCIL, 1, 0);
        c.set_tla_terms(acc(TLA), terms(Some(0), Some(0), None))
            .unwrap();
        ctx(ALICE, 0, 0);
        c.activate_tla(acc(TLA)).unwrap();

        ctx(ALICE, 0, 1);
        assert!(
            c.renew_tla(acc(TLA)).is_ok(),
            "a partner on free terms must not be billed the schedule at renewal"
        );
    }
}

mod business {
    use super::*;

    fn deploy_with_business_tla() -> TlaRegistry {
        let mut c = deploy_priced();
        ctx(ADMIN, 1, 0);
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
    fn an_unbound_authority_cannot_mint_under_a_business_tla() {
        let mut c = deploy_with_business_tla();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(CAROL)).unwrap();
        ctx(CAROL, creation, 1);
        assert!(
            matches!(
                c.rent_sub_account_paid(
                    acc(TLA),
                    "staff".to_string(),
                    acc(BOB),
                    acc(ALICE),
                    "ord-unbound".to_string()
                ),
                Err(ContractError::AuthorityNotBoundToTla)
            ),
            "naming the licensee as payout_account is not authorisation from them"
        );
        assert!(c.get_sub_account(acc(TLA), "staff".to_string()).is_none());
    }

    fn bound_relay() -> TlaRegistry {
        let mut c = deploy_with_business_tla();
        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(CAROL)).unwrap();
        c.bind_payment_authority_tla(acc(CAROL), acc(TLA), U64(50))
            .unwrap();
        c
    }

    #[test]
    fn a_paid_mint_that_never_landed_leaves_its_order_spendable_again() {
        let mut c = bound_relay();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;

        ctx(CAROL, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "staff".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-1".to_string(),
            )
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_sub_account_created_paid(
            settled_for_order("staff", BOB, CAROL, "ord-1"),
            Ok(MintOutcome::CreationFailed),
        );

        ctx(CAROL, creation, 2);
        assert!(
            c.rent_sub_account_paid(
                acc(TLA),
                "staff".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-1".to_string(),
            )
            .is_ok(),
            "the customer paid for this order once and the mint never happened, so it must be retryable"
        );
    }

    #[test]
    fn a_paid_mint_that_landed_can_never_be_spent_twice() {
        let mut c = bound_relay();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;

        ctx(CAROL, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "staff".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-2".to_string(),
            )
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_created_paid(
            settled_for_order("staff", BOB, CAROL, "ord-2"),
            Ok(MintOutcome::Active),
        );

        ctx(CAROL, creation, 2);
        assert!(
            matches!(
                c.rent_sub_account_paid(
                    acc(TLA),
                    "other".to_string(),
                    acc(BOB),
                    acc(BOB),
                    "ord-2".to_string(),
                ),
                Err(ContractError::PaidOrderAlreadySettled)
            ),
            "one settled payment must never buy a second name"
        );
    }

    #[test]
    fn an_order_in_flight_blocks_a_second_attempt_on_the_same_order() {
        let mut c = bound_relay();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;

        ctx(CAROL, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "staff".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-3".to_string(),
            )
            .unwrap();

        ctx(CAROL, creation, 1);
        assert!(
            matches!(
                c.rent_sub_account_paid(
                    acc(TLA),
                    "other".to_string(),
                    acc(BOB),
                    acc(BOB),
                    "ord-3".to_string(),
                ),
                Err(ContractError::PaidOrderAlreadySettled)
            ),
            "two mints in one block must not both draw on the same order"
        );
    }

    #[test]
    fn the_admin_hatch_frees_a_stuck_order_but_never_a_settled_one() {
        let mut c = bound_relay();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;

        ctx(CAROL, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "staff".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-4".to_string(),
            )
            .unwrap();

        ctx(BOB, 1, 2);
        assert!(
            c.admin_release_paid_order("ord-4".to_string()).is_err(),
            "the hatch is admin only"
        );

        ctx(ADMIN, 1, 2);
        assert!(
            matches!(
                c.admin_release_paid_order("ord-4".to_string()),
                Err(ContractError::PaidOrderStillInFlight)
            ),
            "an order may not be released while an honest callback could still land"
        );

        ctx(ADMIN, 1, crate::admin::MANUAL_ORDER_RELEASE_AFTER_NS + 1);
        assert!(c.admin_release_paid_order("ord-4".to_string()).is_ok());
        assert!(
            matches!(
                c.admin_release_paid_order("ord-4".to_string()),
                Err(ContractError::PaidOrderNotFound)
            ),
            "releasing twice must not pretend to have done something"
        );

        ctx(CAROL, creation, 3);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "staff2".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-4".to_string(),
            )
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_created_paid(
            settled_for_order("staff2", BOB, CAROL, "ord-4"),
            Ok(MintOutcome::Active),
        );

        ctx(ADMIN, 1, 4);
        assert!(
            matches!(
                c.admin_release_paid_order("ord-4".to_string()),
                Err(ContractError::PaidOrderAlreadySettled)
            ),
            "the hatch must never reopen a payment that already bought a name"
        );
    }

    fn parked_business_name(c: &mut TlaRegistry, name: &str) {
        rent_business_sub(c, name);
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), name.to_string(), acc(ALICE), Ok(true));
        assert!(c.is_name_re_rentable(acc(TLA), name.to_string()));
    }

    #[test]
    fn a_paid_re_rent_pays_the_licensee_rather_than_the_incoming_owner() {
        let mut c = bound_relay();
        parked_business_name(&mut c, "staff");

        ctx(CAROL, 0, 2);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "staff".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-rr".to_string(),
            )
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        let _ =
            c.on_sub_account_re_rented(settled_for_order("staff", BOB, CAROL, "ord-rr"), Ok(true));

        assert_eq!(
            c.get_sub_account(acc(TLA), "staff".to_string())
                .unwrap()
                .owner,
            acc(BOB)
        );
        assert_eq!(
            payout_of(&c, "staff"),
            acc(ALICE),
            "a re-rent that repointed the payout at the renter would divert the licensee's revenue"
        );

        ctx(CAROL, 0, 3);
        assert!(
            matches!(
                c.rent_sub_account_paid(
                    acc(TLA),
                    "staff2".to_string(),
                    acc(BOB),
                    acc(ALICE),
                    "ord-rr".to_string(),
                ),
                Err(ContractError::PaidOrderAlreadySettled)
            ),
            "a settled re-rent order must be as spent as a settled mint order"
        );
    }

    #[test]
    fn a_failed_paid_re_rent_returns_both_the_order_and_the_mint_slot() {
        let mut c = bound_relay();
        parked_business_name(&mut c, "staff");

        ctx(CAROL, 0, 2);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "staff".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-rr".to_string(),
            )
            .unwrap();
        assert_eq!(c.payment_authority_used(acc(CAROL), acc(TLA)).0, 1);

        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        let _ =
            c.on_sub_account_re_rented(settled_for_order("staff", BOB, CAROL, "ord-rr"), Ok(false));

        assert_eq!(
            c.payment_authority_used(acc(CAROL), acc(TLA)).0,
            0,
            "a re-rent the wallet refused must not burn an allowance slot the relay never spent"
        );
        assert!(c.is_name_re_rentable(acc(TLA), "staff".to_string()));

        ctx(CAROL, 0, 3);
        assert!(
            c.rent_sub_account_paid(
                acc(TLA),
                "staff".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-rr".to_string(),
            )
            .is_ok(),
            "the customer's payment bought nothing, so the order must be spendable again"
        );
    }

    #[test]
    fn a_late_failure_callback_cannot_release_the_order_that_replaced_it() {
        let mut c = bound_relay();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;
        let past_hatch = crate::admin::MANUAL_ORDER_RELEASE_AFTER_NS + 1;

        ctx(CAROL, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "staff".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-5".to_string(),
            )
            .unwrap();
        assert_eq!(c.payment_authority_used(acc(CAROL), acc(TLA)).0, 1);

        ctx(ADMIN, 1, past_hatch);
        c.admin_release_paid_order("ord-5".to_string()).unwrap();
        assert_eq!(
            c.payment_authority_used(acc(CAROL), acc(TLA)).0,
            0,
            "the hatch frees an order whose mint will never land, so it must free the \
             allowance slot that order took as well"
        );

        ctx(CAROL, creation, past_hatch + 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "staff2".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-5".to_string(),
            )
            .unwrap();
        assert_eq!(c.payment_authority_used(acc(CAROL), acc(TLA)).0, 1);

        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_sub_account_created_paid(
            settled_for_order("staff", BOB, CAROL, "ord-5"),
            Ok(MintOutcome::CreationFailed),
        );

        assert_eq!(
            c.payment_authority_used(acc(CAROL), acc(TLA)).0,
            1,
            "the stale callback belongs to a reservation that is already gone, so it \
             must not free the slot the live reservation is holding"
        );
        ctx(CAROL, creation, past_hatch + 2);
        assert!(
            matches!(
                c.rent_sub_account_paid(
                    acc(TLA),
                    "other".to_string(),
                    acc(BOB),
                    acc(ALICE),
                    "ord-5".to_string(),
                ),
                Err(ContractError::PaidOrderAlreadySettled)
            ),
            "the live reservation must still hold the order id after the stale callback"
        );
    }

    #[test]
    fn unbinding_stops_a_relay_that_was_previously_allowed() {
        let mut c = deploy_with_business_tla();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(CAROL)).unwrap();
        c.bind_payment_authority_tla(acc(CAROL), acc(TLA), U64(50))
            .unwrap();
        assert!(c.is_payment_authority_bound(acc(CAROL), acc(TLA)));
        ctx(ADMIN, 1, 1);
        c.unbind_payment_authority_tla(acc(CAROL), acc(TLA))
            .unwrap();
        assert!(!c.is_payment_authority_bound(acc(CAROL), acc(TLA)));
        ctx(CAROL, creation, 1);
        assert!(matches!(
            c.rent_sub_account_paid(
                acc(TLA),
                "staff".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-unbound-2".to_string()
            ),
            Err(ContractError::AuthorityNotBoundToTla)
        ));
    }

    #[test]
    fn a_bound_relay_cannot_mint_past_its_allowance() {
        let mut c = deploy_with_business_tla();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(CAROL)).unwrap();
        c.bind_payment_authority_tla(acc(CAROL), acc(TLA), U64(1))
            .unwrap();

        ctx(CAROL, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "one".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-one".to_string(),
            )
            .expect("the first mint is inside the allowance");
        assert_eq!(c.payment_authority_used(acc(CAROL), acc(TLA)).0, 1);

        ctx(CAROL, creation, 1);
        assert!(
            matches!(
                c.rent_sub_account_paid(
                    acc(TLA),
                    "two".to_string(),
                    acc(BOB),
                    acc(ALICE),
                    "ord-two".to_string()
                ),
                Err(ContractError::AuthorityMintAllowanceSpent)
            ),
            "a compromised relay cannot squat the namespace past what the council allowed"
        );
        assert!(c.get_sub_account(acc(TLA), "two".to_string()).is_none());
    }

    #[test]
    fn a_relay_nearing_its_ceiling_says_so_on_chain() {
        let mut c = deploy_with_business_tla();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(CAROL)).unwrap();
        c.bind_payment_authority_tla(acc(CAROL), acc(TLA), U64(5))
            .unwrap();

        ctx(CAROL, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "first".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-a".to_string(),
            )
            .unwrap();
        assert!(
            near_sdk::test_utils::get_logs()
                .iter()
                .any(|l| l.contains("payment_authority_allowance_low")),
            "the council has to see a relay running out before it stops minting"
        );
    }

    #[test]
    fn a_relay_bound_with_no_allowance_is_refused_rather_than_left_unlimited() {
        let mut c = deploy_with_business_tla();
        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(CAROL)).unwrap();
        assert!(
            matches!(
                c.bind_payment_authority_tla(acc(CAROL), acc(TLA), U64(0)),
                Err(ContractError::AuthorityMintAllowanceZero)
            ),
            "zero must not read as an uncapped bind"
        );
        assert!(!c.is_payment_authority_bound(acc(CAROL), acc(TLA)));
    }

    #[test]
    fn one_settled_order_cannot_be_replayed_into_a_second_name() {
        let mut c = deploy_with_business_tla();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(CAROL)).unwrap();
        c.bind_payment_authority_tla(acc(CAROL), acc(TLA), U64(50))
            .unwrap();

        ctx(CAROL, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "first".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-7".to_string(),
            )
            .expect("the order settles the first name");

        ctx(CAROL, creation, 1);
        assert!(
            matches!(
                c.rent_sub_account_paid(
                    acc(TLA),
                    "second".to_string(),
                    acc(BOB),
                    acc(ALICE),
                    "ord-7".to_string()
                ),
                Err(ContractError::PaidOrderAlreadySettled)
            ),
            "one payment must buy one name"
        );
        assert!(c.get_sub_account(acc(TLA), "second".to_string()).is_none());
    }

    #[test]
    fn a_paid_mint_must_name_the_order_it_settles() {
        let mut c = deploy_with_business_tla();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(CAROL)).unwrap();
        c.bind_payment_authority_tla(acc(CAROL), acc(TLA), U64(50))
            .unwrap();
        ctx(CAROL, creation, 1);
        assert!(matches!(
            c.rent_sub_account_paid(
                acc(TLA),
                "first".to_string(),
                acc(BOB),
                acc(ALICE),
                String::new()
            ),
            Err(ContractError::InvalidOrderId)
        ));
        ctx(CAROL, creation, 1);
        assert!(
            matches!(
                c.rent_sub_account_paid(
                    acc(TLA),
                    "first".to_string(),
                    acc(BOB),
                    acc(ALICE),
                    "x".repeat(crate::rental::MAX_ORDER_ID_LEN + 1)
                ),
                Err(ContractError::InvalidOrderId)
            ),
            "an unbounded identifier is storage a relay can spend for free"
        );
    }

    fn rent_employee_sub(c: &mut TlaRegistry, name: &str, employee: &str) {
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(CAROL)).unwrap();
        c.bind_payment_authority_tla(acc(CAROL), acc(TLA), U64(50))
            .unwrap();
        ctx(CAROL, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                name.to_string(),
                acc(employee),
                acc(ALICE),
                format!("ord-{name}"),
            )
            .unwrap();
    }

    #[test]
    fn the_licensee_can_retract_an_account_an_employee_owns() {
        let mut c = deploy_with_business_tla();
        rent_employee_sub(&mut c, "staff", BOB);
        assert_eq!(
            c.get_sub_account(acc(TLA), "staff".to_string())
                .unwrap()
                .owner,
            acc(BOB),
            "the employee holds it so their wallet can sign as the account"
        );
        ctx(ALICE, 1, 2);
        let _ = c
            .schedule_retraction(acc(TLA), "staff".to_string())
            .unwrap();
        assert!(c.get_retraction_at(acc(TLA), "staff".to_string()).is_some());
        ctx(ALICE, 1, 3);
        let _ = c.cancel_retraction(acc(TLA), "staff".to_string()).unwrap();
        assert!(
            c.get_retraction_at(acc(TLA), "staff".to_string()).is_some(),
            "the registry must not clear the retraction until the wallet lease is restored"
        );
        ctx_callback(near_sdk::PromiseResult::Successful(Vec::new()));
        c.on_retraction_canceled(format!("staff.{TLA}"), acc(ALICE));
        assert!(c.get_retraction_at(acc(TLA), "staff".to_string()).is_none());
    }

    #[test]
    fn a_cancel_the_wallet_refuses_leaves_the_retraction_standing() {
        let mut c = deploy_with_business_tla();
        rent_employee_sub(&mut c, "staff", BOB);
        ctx(ALICE, 1, 2);
        let _ = c
            .schedule_retraction(acc(TLA), "staff".to_string())
            .unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(Vec::new()));
        c.on_retraction_scheduled(format!("staff.{TLA}"), acc(ALICE));
        ctx(ALICE, 1, 3);
        let _ = c.cancel_retraction(acc(TLA), "staff".to_string()).unwrap();
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_retraction_canceled(format!("staff.{TLA}"), acc(ALICE));
        assert!(
            c.get_retraction_at(acc(TLA), "staff".to_string()).is_some(),
            "a failed lease restore must not leave the registry claiming the lease is safe"
        );
    }

    #[test]
    fn a_schedule_the_wallet_refuses_rolls_the_registry_back() {
        let mut c = deploy_with_business_tla();
        rent_employee_sub(&mut c, "staff", BOB);
        ctx(ALICE, 1, 2);
        let _ = c
            .schedule_retraction(acc(TLA), "staff".to_string())
            .unwrap();
        assert!(c.get_retraction_at(acc(TLA), "staff".to_string()).is_some());
        ctx_callback(near_sdk::PromiseResult::Failed);
        c.on_retraction_scheduled(format!("staff.{TLA}"), acc(ALICE));
        assert!(
            c.get_retraction_at(acc(TLA), "staff".to_string()).is_none(),
            "a failed lease shortening must not leave a retraction the wallet never applied"
        );
    }

    #[test]
    fn an_elapsed_retraction_reads_reclaimable_to_a_client() {
        let mut c = deploy_with_business_tla();
        rent_employee_sub(&mut c, "staff", BOB);
        ctx(ALICE, 1, 2);
        let _ = c
            .schedule_retraction(acc(TLA), "staff".to_string())
            .unwrap();

        let notice = c.get_fee_config().retraction_notice_ns.0;
        let after = (2 + notice).max(GRACE_NS) + 1;
        ctx(BOB, 0, after);
        let view = c.get_sub_account(acc(TLA), "staff".to_string()).unwrap();
        assert!(
            matches!(view.lifecycle, LifecycleStatus::Reclaimable),
            "the view must report what assert_sellable and resolve_reclaimable enforce"
        );
        assert!(
            view.expires_at.0 > after,
            "its own term has not run, so only the retraction explains the status"
        );
    }

    #[test]
    fn a_stranger_still_cannot_retract_a_business_account() {
        let mut c = deploy_with_business_tla();
        rent_employee_sub(&mut c, "staff", BOB);
        ctx("mallory.testnet", 1, 2);
        assert!(matches!(
            c.schedule_retraction(acc(TLA), "staff".to_string()),
            Err(ContractError::OnlyLicensee)
        ));
    }

    #[test]
    fn an_employee_cannot_schedule_their_own_retraction() {
        let mut c = deploy_with_business_tla();
        rent_employee_sub(&mut c, "staff", BOB);
        ctx(BOB, 1, 2);
        assert!(
            matches!(
                c.schedule_retraction(acc(TLA), "staff".to_string()),
                Err(ContractError::OnlyLicensee)
            ),
            "scheduling and cancelling must sit with the same party: an employee who could start \
             the notice period but not stop it can lose the name to a licensee who never looks"
        );
    }

    #[test]
    fn a_business_employee_cannot_redirect_the_licensee_payout() {
        let mut c = deploy_with_business_tla();
        let creation = c.get_fee_config().account_creation_deposit_yocto.0;
        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(CAROL)).unwrap();
        c.bind_payment_authority_tla(acc(CAROL), acc(TLA), U64(50))
            .unwrap();
        ctx(CAROL, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "staff".to_string(),
                acc(BOB),
                acc(ALICE),
                "ord-staff".to_string(),
            )
            .unwrap();
        assert_eq!(payout_of(&c, "staff"), acc(ALICE));

        ctx(BOB, 1, 2);
        assert!(
            matches!(
                c.set_payout_account(acc(TLA), "staff".to_string(), acc(BOB)),
                Err(ContractError::OnlyLicensee)
            ),
            "the employee holds the name but the licence pays for it"
        );
        ctx(ALICE, 1, 2);
        assert!(c
            .set_payout_account(acc(TLA), "staff".to_string(), acc(CAROL))
            .is_ok());
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
        ctx(ADMIN, 1, 1);
        c.set_business_sub_cap(acc(TLA), Some(7)).unwrap();
        assert_eq!(c.get_business_sub_cap(acc(TLA)), 7);
        ctx(ADMIN, 1, 1);
        c.set_business_sub_cap(acc(TLA), None).unwrap();
        assert_eq!(
            c.get_business_sub_cap(acc(TLA)),
            default_cap,
            "clearing the override must fall back to the configured default"
        );
    }

    #[test]
    fn business_renewal_cost_quotes_the_tla_rent_only() {
        let mut c = deploy_with_business_tla();
        let view = c.get_business_renewal_cost(acc(TLA)).unwrap();
        assert_eq!(view.tla_id, acc(TLA));
        assert!(view.tla_rent_yocto.0 > 0);

        rent_business_sub(&mut c, "staff");
        let with_a_sub = c.get_business_renewal_cost(acc(TLA)).unwrap();
        assert_eq!(
            with_a_sub.tla_rent_yocto, view.tla_rent_yocto,
            "renew_tla charges base rent alone, so the quote cannot move with the sub-account count"
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
        ctx(ADMIN, 1, 1);
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
        let _ = c
            .schedule_retraction(acc(TLA), "staff".to_string())
            .unwrap();
        assert!(c.get_retraction_at(acc(TLA), "staff".to_string()).is_some());
        ctx(ALICE, 1, 3);
        let _ = c.cancel_retraction(acc(TLA), "staff".to_string()).unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(Vec::new()));
        c.on_retraction_canceled(format!("staff.{TLA}"), acc(ALICE));
        assert!(c.get_retraction_at(acc(TLA), "staff".to_string()).is_none());
    }

    #[test]
    fn a_business_sub_cannot_be_traded_by_any_path() {
        let mut c = deploy_with_business_tla();
        rent_business_sub(&mut c, "staff");
        let key = format!("staff.{TLA}");
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.nft_transfer(acc(BOB), key.clone(), None, None),
            Err(ContractError::BusinessSubNotResellable)
        ));
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.nft_transfer_call(acc(BOB), key, None, None, String::new()),
            Err(ContractError::BusinessSubNotResellable)
        ));
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.transfer_sub_account(acc(TLA), "staff".to_string(), acc(BOB)),
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
        ctx(ADMIN, 1, 1);
        c.admin_set_initial_rate(U128(dollars(5))).unwrap();
        ctx(ADMIN, 1, 1);
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
        ctx(ADMIN, 1, 0);
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
        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.admin_set_initial_rate(U128(dollars(1_000))),
            Err(ContractError::RateOutOfBounds)
        ));
    }

    #[test]
    fn initial_rate_cannot_be_set_twice() {
        let mut c = deploy_initialized();
        ctx(ADMIN, 1, 1);
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
        ctx(ADMIN, 1, 1 + COOLDOWN);
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
        ctx(ADMIN, 1, 1);
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
        ctx(ADMIN, 1, 1);
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
        ctx(ADMIN, 1, 1);
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
        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.update_fee_config(zero_cap),
            Err(ContractError::InvalidBusinessCap)
        ));

        let mut short_notice = base.clone();
        short_notice.retraction_notice_ns = U64(1);
        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.update_fee_config(short_notice),
            Err(ContractError::RetractionNoticeTooShort)
        ));

        let mut age_below_cooldown = base.clone();
        age_below_cooldown.max_rate_age_ns = U64(1);
        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.update_fee_config(age_below_cooldown),
            Err(ContractError::InvalidRateBounds)
        ));

        let mut no_cooldown = base.clone();
        no_cooldown.rate_update_cooldown_ns = U64(0);
        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.update_fee_config(no_cooldown),
            Err(ContractError::InvalidRateBounds)
        ));

        let mut huge_fee = base;
        huge_fee.rent_tier_5_usd_micro = U128(crate::pricing::MAX_USD_MICRO + 1);
        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.update_fee_config(huge_fee),
            Err(ContractError::FeeExceedsCap)
        ));
    }

    #[test]
    fn config_rejects_inverted_rate_bounds() {
        let mut c = deploy();
        ctx(ADMIN, 1, 1);
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
        ctx(ADMIN, 1, 1);
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
        ctx(ADMIN, 1, 0);
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
        ctx(ADMIN, 1, 0);
        TlaRegistry::new(
            acc(ADMIN),
            acc(HOSEXT),
            U64(GRACE_NS),
            acc(TREASURY),
            acc(OTHER_COUNCIL),
            None,
        )
    }

    #[test]
    fn an_operations_admin_cannot_escalate_itself() {
        let mut c = deploy_split();
        ctx(ADMIN, 1, 0);
        assert!(matches!(
            c.add_admin(acc(BOB)),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn an_operations_admin_cannot_move_the_fee_model_or_release_revenue() {
        let mut c = deploy_split();
        ctx(ADMIN, 1, 0);
        let config = c.get_fee_config();
        assert!(matches!(
            c.update_fee_config(config),
            Err(ContractError::OnlyCouncil)
        ));
        assert!(matches!(
            c.withdraw(U128(1)),
            Err(ContractError::OnlyTreasuryOrCouncil)
        ));
    }

    #[test]
    fn the_treasury_releases_revenue_without_a_council_vote() {
        let mut c = deploy_split();
        c.total_revenue = 500;
        ctx(TREASURY, 1, 1);
        c.withdraw(U128(500)).unwrap();
        assert_eq!(
            c.get_pending_refund(acc(TREASURY)).0,
            500,
            "the treasury must be able to release its own revenue on its own authority"
        );
        assert_eq!(c.get_stats().total_revenue_yocto.0, 0);
    }

    #[test]
    fn revenue_still_only_ever_lands_on_the_treasury() {
        let mut c = deploy_split();
        c.total_revenue = 500;
        ctx(OTHER_COUNCIL, 1, 1);
        c.withdraw(U128(500)).unwrap();
        assert_eq!(
            c.get_pending_refund(acc(TREASURY)).0,
            500,
            "a council withdrawal credits the treasury, never the caller"
        );
        assert_eq!(c.get_pending_refund(acc(OTHER_COUNCIL)).0, 0);
    }

    #[test]
    fn an_outsider_cannot_release_revenue() {
        let mut c = deploy_split();
        c.total_revenue = 500;
        ctx(BOB, 1, 1);
        assert!(matches!(
            c.withdraw(U128(500)),
            Err(ContractError::OnlyTreasuryOrCouncil)
        ));
        assert_eq!(c.get_stats().total_revenue_yocto.0, 500);
    }

    fn rotate_treasury_to(c: &mut TlaRegistry, next: &str) {
        ctx(OTHER_COUNCIL, 1, 1);
        c.approve_treasury_rotation(acc(next)).unwrap();
        ctx(next, 1, 1);
        c.commit_treasury_rotation().unwrap();
    }

    #[test]
    fn a_rotation_takes_effect_only_once_the_incoming_treasury_claims_it() {
        let mut c = deploy_split();
        ctx(OTHER_COUNCIL, 1, 1);
        c.approve_treasury_rotation(acc(BOB)).unwrap();
        assert_eq!(
            c.get_treasury(),
            acc(TREASURY),
            "approving a rotation must not move the destination on its own"
        );
        assert_eq!(c.pending_treasury(), Some(acc(BOB)));
        ctx(BOB, 1, 1);
        c.commit_treasury_rotation().unwrap();
        assert_eq!(c.get_treasury(), acc(BOB));
        assert_eq!(c.pending_treasury(), None);
    }

    #[test]
    fn an_address_that_cannot_sign_never_becomes_the_treasury() {
        let mut c = deploy_split();
        ctx(OTHER_COUNCIL, 1, 1);
        c.approve_treasury_rotation(acc(BOB)).unwrap();
        ctx(OTHER_COUNCIL, 1, 1);
        assert!(matches!(
            c.commit_treasury_rotation(),
            Err(ContractError::OnlyPendingTreasury)
        ));
        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.commit_treasury_rotation(),
            Err(ContractError::OnlyPendingTreasury)
        ));
        assert_eq!(c.get_treasury(), acc(TREASURY));
    }

    #[test]
    fn the_council_can_cancel_a_rotation_before_it_is_claimed() {
        let mut c = deploy_split();
        ctx(OTHER_COUNCIL, 1, 1);
        c.approve_treasury_rotation(acc(BOB)).unwrap();
        c.cancel_treasury_rotation().unwrap();
        assert_eq!(c.pending_treasury(), None);
        ctx(BOB, 1, 1);
        assert!(matches!(
            c.commit_treasury_rotation(),
            Err(ContractError::NoTreasuryRotationPending)
        ));
        assert_eq!(c.get_treasury(), acc(TREASURY));
    }

    #[test]
    fn the_treasury_cannot_rotate_itself() {
        let mut c = deploy_split();
        ctx(TREASURY, 1, 1);
        assert!(matches!(
            c.approve_treasury_rotation(acc(BOB)),
            Err(ContractError::OnlyCouncil)
        ));
        assert_eq!(
            c.get_treasury(),
            acc(TREASURY),
            "withdrawing freely must not let the treasury move the destination"
        );
    }

    #[test]
    fn an_operations_admin_cannot_rotate_the_treasury() {
        let mut c = deploy_split();
        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.approve_treasury_rotation(acc(BOB)),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn the_treasury_cannot_be_rotated_to_a_no_op_or_to_the_registry() {
        let mut c = deploy_split();
        ctx(OTHER_COUNCIL, 1, 1);
        assert!(matches!(
            c.approve_treasury_rotation(acc(TREASURY)),
            Err(ContractError::TreasuryUnchanged)
        ));
        assert!(matches!(
            c.approve_treasury_rotation(near_sdk::env::current_account_id()),
            Err(ContractError::TreasuryIsSelf)
        ));
    }

    #[test]
    fn revenue_released_after_a_rotation_lands_on_the_new_treasury() {
        let mut c = deploy_split();
        c.total_revenue = 500;
        rotate_treasury_to(&mut c, BOB);
        ctx(BOB, 1, 1);
        c.withdraw(U128(500)).unwrap();
        assert_eq!(c.get_pending_refund(acc(BOB)).0, 500);
        assert_eq!(
            c.get_pending_refund(acc(TREASURY)).0,
            0,
            "the outgoing treasury must not be credited after it is rotated out"
        );
    }

    #[test]
    fn the_outgoing_treasury_can_no_longer_release_revenue() {
        let mut c = deploy_split();
        c.total_revenue = 500;
        rotate_treasury_to(&mut c, BOB);
        ctx(TREASURY, 1, 1);
        assert!(matches!(
            c.withdraw(U128(500)),
            Err(ContractError::OnlyTreasuryOrCouncil)
        ));
    }

    #[test]
    fn an_operations_admin_cannot_grant_authorities_or_open_a_tla() {
        let mut c = deploy_split();
        ctx(ADMIN, 1, 0);
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
        ctx(OTHER_COUNCIL, 1, 0);
        c.add_admin(acc(BOB)).unwrap();
        c.add_payment_authority(acc(BOB)).unwrap();
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
    }

    #[test]
    fn operations_keeps_pause_and_unpause_so_an_incident_needs_no_multisig() {
        let mut c = deploy_split();
        ctx(ADMIN, 1, 0);
        c.pause().unwrap();
        assert!(c.is_paused());
        c.unpause().unwrap();
        assert!(!c.is_paused());
    }

    #[test]
    fn the_council_cannot_run_operations() {
        let mut c = deploy_split();
        ctx(OTHER_COUNCIL, 1, 0);
        assert!(matches!(c.pause(), Err(ContractError::OnlyAdmin)));
    }

    #[test]
    fn only_the_council_removes_an_admin_and_never_the_last_one() {
        let mut c = deploy_split();
        ctx(OTHER_COUNCIL, 1, 0);
        c.add_admin(acc(BOB)).unwrap();

        ctx(ADMIN, 1, 0);
        assert!(
            matches!(c.remove_admin(acc(BOB)), Err(ContractError::OnlyCouncil)),
            "an admin who can strip the admin set can lock the council out of its own operations"
        );

        ctx(OTHER_COUNCIL, 1, 0);
        c.remove_admin(acc(BOB)).unwrap();
        assert!(matches!(
            c.remove_admin(acc(ADMIN)),
            Err(ContractError::CannotRemoveLastAdmin)
        ));
        assert_eq!(c.get_admins(), vec![acc(ADMIN)]);
    }

    #[test]
    fn only_an_admin_unsuspends_a_tla() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 1, 1);
        c.suspend_tla(acc(TLA)).unwrap();

        ctx("mallory.testnet", 1, 1);
        assert!(matches!(
            c.unsuspend_tla(acc(TLA)),
            Err(ContractError::OnlyAdmin)
        ));

        ctx(ADMIN, 1, 1);
        c.unsuspend_tla(acc(TLA)).unwrap();
        assert!(matches!(
            c.unsuspend_tla(acc(TLA)),
            Err(ContractError::TlaNotSuspended)
        ));
        assert_eq!(c.get_suspension_expiry(acc(TLA)).0, 0);
    }
}

mod marketplace_pause {
    use super::*;

    fn pause_market(c: &mut TlaRegistry) {
        ctx(ADMIN, 1, 2);
        c.pause_marketplace().unwrap();
    }

    #[test]
    fn a_paused_marketplace_refuses_a_new_deposit() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        pause_market(&mut c);
        ctx(ALICE, 1, 2);
        assert!(matches!(
            c.nft_transfer_call(acc(BOB), format!("alice.{TLA}"), None, None, String::new()),
            Err(ContractError::MarketplacePaused)
        ));
    }

    #[test]
    fn a_paused_marketplace_never_traps_a_deposited_name() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        settle_transfer(&mut c, "alice", ALICE, BOB);
        pause_market(&mut c);
        ctx(BOB, 1, 2);
        assert!(
            c.nft_transfer(acc(ALICE), format!("alice.{TLA}"), None, None)
                .is_ok(),
            "a market pause must never block the exit, or a holder cannot return a name"
        );
    }

    #[test]
    fn a_paused_marketplace_still_allows_a_direct_transfer() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        pause_market(&mut c);
        ctx(ALICE, 1, 2);
        assert!(
            c.transfer_sub_account(acc(TLA), "alice".to_string(), acc(BOB))
                .is_ok(),
            "the registry pause, not the market pause, gates a direct transfer"
        );
    }

    #[test]
    fn the_registry_pause_stops_every_path_including_the_exit() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ADMIN, 1, 2);
        c.pause().unwrap();
        ctx(ALICE, 1, 3);
        assert!(matches!(
            c.nft_transfer(acc(BOB), format!("alice.{TLA}"), None, None),
            Err(ContractError::Paused)
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
    fn unpausing_restores_trading() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        pause_market(&mut c);
        ctx(ADMIN, 1, 2);
        c.unpause_marketplace().unwrap();
        ctx(ALICE, 1, 2);
        assert!(c
            .nft_transfer(acc(BOB), format!("alice.{TLA}"), None, None)
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
        ctx(ADMIN, 1, at);
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
        ctx(ADMIN, 1, expires + GRACE_NS);
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
        ctx(ADMIN, 1, long_after);
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
        ctx(ADMIN, 1, 1);
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
        ctx(ADMIN, 1, 1);
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
        ctx(ADMIN, 1, 2);
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
        ctx(ADMIN, 1, 2);
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
        ctx(ADMIN, 1, 1);
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
            expires + GRACE_NS + DAY_NS,
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
        ctx(ADMIN, 1, 1);
        c.pause().unwrap();
        ctx(BOB, rent_total(&c, "bob"), 1);
        assert!(matches!(
            c.rent_sub_account(acc(TLA), "bob".to_string(), None),
            Err(ContractError::Paused)
        ));
    }
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
    fn tokens_are_enumerable_by_owner() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "bob");
        assert_eq!(c.nft_total_supply().0, 2);
        assert_eq!(c.nft_supply_for_owner(acc(ALICE)).0, 2);
        let held = c.nft_tokens_for_owner(acc(ALICE), None, None);
        assert_eq!(held.len(), 2);
        assert!(held.iter().all(|t| t.owner_id == acc(ALICE)));
    }

    #[test]
    fn token_enumeration_respects_offset_and_limit() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        rent_alice_sub(&mut c, "bob");
        assert_eq!(c.nft_tokens(None, Some(1)).len(), 1);
        assert_eq!(c.nft_tokens(Some(U128(1)), Some(10)).len(), 1);
        assert_eq!(c.nft_tokens(Some(U128(2)), Some(10)).len(), 0);
    }

    #[test]
    fn token_enumeration_refuses_an_unbounded_scan() {
        let mut c = deploy_with_open_tla();
        for i in 0..3 {
            rent_alice_sub(&mut c, &format!("name{i}"));
        }
        assert_eq!(
            c.nft_tokens(None, Some(u64::MAX)).len(),
            3,
            "an oversized limit is clamped, not honoured"
        );
        assert_eq!(
            c.nft_tokens_for_owner(acc(ALICE), None, Some(u64::MAX))
                .len(),
            3
        );
    }

    #[test]
    fn the_collection_answers_nep177_from_the_moment_it_exists() {
        let c = deploy_with_open_tla();
        let meta = c.nft_metadata();
        assert_eq!(
            meta.spec, NFT_SPEC,
            "a wallet that validates the spec must accept it"
        );
        assert!(!meta.name.is_empty(), "a nameless collection is skipped");
        assert!(!meta.symbol.is_empty());
    }

    #[test]
    fn admin_branding_replaces_the_placeholder() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 1, 1);
        c.admin_set_nft_metadata(
            "collection".to_string(),
            "sym".to_string(),
            None,
            None,
            None,
        )
        .unwrap();
        let meta = c.nft_metadata();
        assert_eq!(meta.name, "collection");
        assert_eq!(meta.symbol, "sym");
        assert_eq!(meta.spec, NFT_SPEC, "the spec is not the admin's to change");
    }

    #[test]
    fn every_token_carries_metadata_a_wallet_can_render() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = format!("alice.{TLA}");

        let token = c.nft_token(key.clone()).unwrap();
        let meta = token.metadata.expect("a nameless token renders blank");
        assert_eq!(
            meta.title,
            Some(key.clone()),
            "the title is the name itself"
        );
        assert_eq!(meta.copies, Some(1), "each name is one of one");

        let extra: near_sdk::serde_json::Value =
            near_sdk::serde_json::from_str(&meta.extra.expect("lease timing travels in extra"))
                .unwrap();
        let sub = c.get_sub_account(acc(TLA), "alice".to_string()).unwrap();
        assert_eq!(extra["expires_at"], sub.expires_at.0.to_string());

        for listed in [
            c.nft_tokens(None, None),
            c.nft_tokens_for_owner(acc(ALICE), None, None),
        ] {
            assert!(
                listed.iter().all(|t| t.metadata.is_some()),
                "enumeration must carry the same metadata a single read does"
            );
        }
    }

    #[test]
    fn token_metadata_follows_a_renewal_rather_than_going_stale() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = format!("alice.{TLA}");
        let before = c.nft_token(key.clone()).unwrap().metadata.unwrap().extra;
        let expires_before = c
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
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_sub_account_renewed(
            acc(TLA),
            "alice".to_string(),
            U64(expires_before + ONE_YEAR_NS),
            acc(ALICE),
            U128(rent),
        );

        let after = c.nft_token(key).unwrap().metadata.unwrap().extra;
        assert_ne!(
            before, after,
            "expiry is derived on read, so a renewal shows up without a write"
        );
    }

    #[test]
    fn empty_registry_pages_are_empty_not_missing() {
        let c = deploy_with_open_tla();
        assert_eq!(c.list_sub_accounts(0, 10).len(), 0);
        assert_eq!(c.nft_total_supply().0, 0);
        assert!(c.nft_tokens(None, None).is_empty());
        assert!(c.nft_tokens_for_owner(acc(ALICE), None, None).is_empty());
        assert!(owned(&c, ALICE).is_empty());
    }

    #[test]
    fn a_page_carries_the_lifecycle_state_so_no_second_call_is_needed() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let page = c.list_sub_accounts(0, 10);
        assert_eq!(page[0].sub_account.full_name, format!("alice.{TLA}"));
        assert_eq!(page[0].sub_account.owner, acc(ALICE));
        assert!(page[0].retraction_at.is_none());
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
        ctx(ADMIN, 1, 1);
        c.add_payment_authority(acc(BOB)).unwrap();
        c.bind_payment_authority_tla(acc(BOB), acc(TLA), U64(50))
            .unwrap();
        ctx(BOB, creation, 1);
        let _ = c
            .rent_sub_account_paid(
                acc(TLA),
                "alice".to_string(),
                acc(ALICE),
                acc(ALICE),
                "ord-paid-alice".to_string(),
            )
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
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE), Ok(true));
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
            RotationCause::Transfer,
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
            RotationCause::Transfer,
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
    fn a_settled_nft_transfer_moves_the_name_to_the_buyer() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.nft_on_rotation_resolved(
            crate::nft::NftRotation {
                tla_id: acc(TLA),
                name: "alice".to_string(),
                from: acc(ALICE),
                to: acc(BOB),
                memo: None,
                cause: RotationCause::Transfer,
            },
            Ok(true),
        );
        assert!(owned(&c, ALICE).is_empty());
        assert_eq!(owned(&c, BOB), vec![format!("alice.{TLA}")]);
        assert_eq!(
            c.nft_token(format!("alice.{TLA}")).unwrap().owner_id,
            acc(BOB)
        );
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
        settle_transfer(&mut c, "alice", ALICE, BOB);
        let feed = c.list_recent_activity(0, 10, None);
        assert_eq!(feed[0].event, "sub_account_transferred");
        assert_eq!(feed[0].account, format!("alice.{TLA}"));
        assert_eq!(feed[1].event, "sub_account_rented");
    }

    #[test]
    fn activity_paging_respects_offset_and_limit() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        settle_transfer(&mut c, "alice", ALICE, BOB);
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
        settle_transfer(&mut c, "alice", ALICE, BOB);
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
            let (from, to) = if i % 2 == 0 {
                (ALICE, BOB)
            } else {
                (BOB, ALICE)
            };
            settle_transfer(&mut c, "alice", from, to);
        }
        let feed = c.list_recent_activity(0, ACTIVITY_CAPACITY as u64 + 50, None);
        assert_eq!(
            feed.len(),
            ACTIVITY_CAPACITY as usize,
            "the buffer stays bounded"
        );
        assert_eq!(
            feed[0].event, "sub_account_transferred",
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
            RotationCause::Transfer,
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
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(ALICE), Ok(true));
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

mod migration {
    use super::*;

    #[test]
    fn the_version_is_the_first_field_so_it_is_readable_before_anything_else() {
        let c = deploy();
        let bytes = near_sdk::borsh::to_vec(&c).unwrap();
        assert_eq!(
            u16::from_le_bytes([bytes[0], bytes[1]]),
            crate::STATE_VERSION
        );
    }

    #[test]
    #[should_panic(expected = "state version")]
    fn migrate_refuses_a_state_version_it_does_not_understand() {
        let mut c = deploy();
        c.state_version = crate::STATE_VERSION + 1;
        ctx("registry.testnet", 0, 0);
        near_sdk::env::state_write(&c);
        crate::TlaRegistry::migrate();
    }

    fn as_v1(c: TlaRegistry) -> crate::legacy::TlaRegistryV1 {
        crate::legacy::TlaRegistryV1 {
            state_version: 1,
            tlas: c.tlas,
            sub_accounts: c.sub_accounts,
            sub_accounts_by_owner: c.sub_accounts_by_owner,
            sub_accounts_by_tla: c.sub_accounts_by_tla,
            recent_activity: c.recent_activity,
            activity_cursor: c.activity_cursor,
            admins: c.admins,
            fee_config: c.fee_config,
            total_revenue: c.total_revenue,
            sub_account_count: c.sub_account_count,
            paused: c.paused,
            version: c.version,
            pending_refunds: c.pending_refunds,
            total_pending_refunds: c.total_pending_refunds,
            ft_allowlist: c.ft_allowlist,
            business_sub_count: c.business_sub_count,
            business_sub_cap_override: c.business_sub_cap_override,
            parked_names: c.parked_names,
            reclaim_pending: c.reclaim_pending,
            payment_authorities: c.payment_authorities,
            recovery_authorities: c.recovery_authorities,
            hos_extension: c.hos_extension,
            grace_period_ns: c.grace_period_ns,
            price_oracle: c.price_oracle,
            near_usd_rate_micro: c.near_usd_rate_micro,
            rate_updated_at: c.rate_updated_at,
            rate_sequence: c.rate_sequence,
            treasury: c.treasury,
            council: c.council,
            marketplace_paused: c.marketplace_paused,
            paused_until_ns: c.paused_until_ns,
            unpaused_at: c.unpaused_at,
            sweepable_tokens: c.sweepable_tokens,
            suspended_until: c.suspended_until,
            nft_contract_metadata: c.nft_contract_metadata,
            approved_code_hash: c.approved_code_hash,
            approved_at: c.approved_at,
            upgrade_delay_ns: c.upgrade_delay_ns,
            venues: c.venues,
            upgrade_proven: c.upgrade_proven,
        }
    }

    #[test]
    fn a_state_left_at_version_one_migrates_through_its_own_reader() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
        let council = c.get_council();
        let old = as_v1(c);
        ctx("registry.testnet", 0, 0);
        near_sdk::env::state_write(&old);
        drop(old);

        let migrated = crate::TlaRegistry::migrate();
        assert_eq!(migrated.state_version, crate::STATE_VERSION);
        assert_eq!(migrated.council, council);
        assert!(
            migrated.tlas.contains_key(&acc(TLA)),
            "the shape changed, the data did not"
        );
        assert_eq!(migrated.lease_term_ns, crate::PRODUCTION_LEASE_TERM_NS);
        assert!(migrated.pending_council.is_none());
    }

    fn as_v4(c: TlaRegistry) -> crate::legacy::TlaRegistryV4 {
        crate::legacy::TlaRegistryV4 {
            state_version: 4,
            tlas: c.tlas,
            sub_accounts: c.sub_accounts,
            sub_accounts_by_owner: c.sub_accounts_by_owner,
            sub_accounts_by_tla: c.sub_accounts_by_tla,
            recent_activity: c.recent_activity,
            activity_cursor: c.activity_cursor,
            admins: c.admins,
            fee_config: c.fee_config,
            total_revenue: c.total_revenue,
            sub_account_count: c.sub_account_count,
            paused: c.paused,
            version: c.version,
            pending_refunds: c.pending_refunds,
            total_pending_refunds: c.total_pending_refunds,
            ft_allowlist: c.ft_allowlist,
            business_sub_count: c.business_sub_count,
            business_sub_cap_override: c.business_sub_cap_override,
            tla_terms: c.tla_terms,
            parked_names: c.parked_names,
            reclaim_pending: c.reclaim_pending,
            payment_authorities: c.payment_authorities,
            recovery_authorities: c.recovery_authorities,
            hos_extension: c.hos_extension,
            grace_period_ns: c.grace_period_ns,
            lease_term_ns: c.lease_term_ns,
            price_oracle: c.price_oracle,
            near_usd_rate_micro: c.near_usd_rate_micro,
            rate_updated_at: c.rate_updated_at,
            rate_sequence: c.rate_sequence,
            treasury: c.treasury,
            council: c.council,
            marketplace_paused: c.marketplace_paused,
            paused_until_ns: c.paused_until_ns,
            unpaused_at: c.unpaused_at,
            sweepable_tokens: c.sweepable_tokens,
            suspended_until: c.suspended_until,
            nft_contract_metadata: c.nft_contract_metadata,
            approved_code_hash: c.approved_code_hash,
            approved_at: c.approved_at,
            upgrade_delay_ns: c.upgrade_delay_ns,
            venues: c.venues,
            upgrade_proven: c.upgrade_proven,
            paid_order_ids: c.paid_order_ids,
            pending_council: c.pending_council,
            pending_council_at: c.pending_council_at,
        }
    }

    #[test]
    fn the_deployed_shape_migrates_and_starts_with_no_rotation_pending() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        c.register_tla(acc(TLA), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
        c.total_revenue = 500;
        let treasury = c.get_treasury();
        let old = as_v4(c);
        ctx("registry.testnet", 0, 0);
        near_sdk::env::state_write(&old);
        drop(old);

        let migrated = crate::TlaRegistry::migrate();
        assert_eq!(migrated.state_version, crate::STATE_VERSION);
        assert_eq!(migrated.treasury, treasury, "the destination must survive");
        assert_eq!(migrated.total_revenue, 500, "revenue must survive");
        assert!(migrated.tlas.contains_key(&acc(TLA)));
        assert!(
            migrated.pending_treasury.is_none(),
            "an upgrade must not arrive with a rotation already half-committed"
        );
    }

    #[test]
    #[should_panic(expected = "state version")]
    fn the_current_reader_is_not_offered_a_shape_it_cannot_read() {
        let c = deploy();
        let mut old = as_v1(c);
        old.state_version = 9;
        ctx("registry.testnet", 0, 0);
        near_sdk::env::state_write(&old);
        crate::TlaRegistry::migrate();
    }
}

mod keyless_upgrade {
    use super::*;
    use crate::admin::UPGRADE_DELAY_NS;
    use near_sdk::json_types::{Base58CryptoHash, Base64VecU8};

    fn code() -> Base64VecU8 {
        Base64VecU8(vec![7u8; 32])
    }

    fn hash() -> Base58CryptoHash {
        Base58CryptoHash::from(near_sdk::env::sha256_array(&code().0))
    }

    #[test]
    fn the_council_can_ship_code_without_any_key_on_the_account() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        c.approve_upgrade(hash()).unwrap();
        assert_eq!(c.approved_upgrade_hash(), Some(hash()));
        ctx(COUNCIL, 1, UPGRADE_DELAY_NS + 1);
        assert!(c.upgrade(code()).is_ok());
        assert_eq!(c.approved_upgrade_hash(), None, "an approval is spent once");
    }

    #[test]
    fn nobody_but_the_council_may_approve() {
        let mut c = deploy();
        ctx(ALICE, 1, 0);
        assert!(matches!(
            c.approve_upgrade(hash()),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn code_that_does_not_match_the_approval_is_refused() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        c.approve_upgrade(hash()).unwrap();
        ctx(COUNCIL, 1, UPGRADE_DELAY_NS + 1);
        assert!(matches!(
            c.upgrade(Base64VecU8(vec![9u8; 32])),
            Err(ContractError::HashMismatch)
        ));
    }

    #[test]
    fn an_approval_must_serve_the_delay() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        c.approve_upgrade(hash()).unwrap();
        ctx(COUNCIL, 1, UPGRADE_DELAY_NS - 1);
        assert!(matches!(
            c.upgrade(code()),
            Err(ContractError::ApprovalTooYoung)
        ));
    }

    #[test]
    fn upgrading_without_an_approval_is_refused() {
        let mut c = deploy();
        ctx(COUNCIL, 1, UPGRADE_DELAY_NS + 1);
        assert!(matches!(
            c.upgrade(code()),
            Err(ContractError::NoApprovedHash)
        ));
    }

    fn a_key() -> near_sdk::PublicKey {
        std::str::FromStr::from_str("ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847").unwrap()
    }

    #[test]
    fn the_council_can_seal_the_registry_once_the_upgrade_path_is_proven() {
        let mut c = deploy();
        c.upgrade_proven = true;
        ctx(COUNCIL, 1, 0);
        assert!(c.seal(a_key()).is_ok());
        let deleted: Vec<String> = near_sdk::test_utils::get_created_receipts()
            .into_iter()
            .flat_map(|receipt| receipt.actions)
            .filter_map(|action| match action {
                near_sdk::mock::MockAction::DeleteKey { public_key, .. } => {
                    Some(public_key.to_string())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            deleted,
            vec![String::from(&a_key())],
            "the launch gate turns on this account ending with no key, so the seal has to \
             schedule the removal rather than only report one"
        );
    }

    #[test]
    fn sealing_the_registry_is_refused_until_an_upgrade_has_run() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        assert!(matches!(
            c.seal(a_key()),
            Err(ContractError::UpgradeNotProven)
        ));
    }

    #[test]
    fn a_council_that_cannot_call_cannot_remove_the_key() {
        let mut c = deploy();
        ctx(BOB, 1, 0);
        assert!(matches!(c.seal(a_key()), Err(ContractError::OnlyCouncil)));
    }

    #[test]
    fn releasing_revenue_needs_a_full_access_signature() {
        let mut c = deploy();
        ctx(COUNCIL, 0, 0);
        assert!(matches!(
            c.withdraw(U128(1)),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn granting_the_recovery_role_needs_a_full_access_signature() {
        let mut c = deploy();
        ctx(COUNCIL, 0, 0);
        assert!(matches!(
            c.add_recovery_authority(acc(BOB)),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn adding_an_admin_needs_a_full_access_signature() {
        let mut c = deploy();
        ctx(COUNCIL, 0, 0);
        assert!(matches!(
            c.add_admin(acc(BOB)),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn changing_fees_needs_a_full_access_signature() {
        let mut c = deploy();
        let config = c.get_fee_config();
        ctx(COUNCIL, 0, 0);
        assert!(matches!(
            c.update_fee_config(config),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn sealing_the_registry_needs_a_full_access_signature() {
        let mut c = deploy();
        ctx(COUNCIL, 0, 0);
        assert!(matches!(
            c.seal(a_key()),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn seeding_the_rate_needs_a_full_access_signature() {
        let mut c = deploy();
        ctx(COUNCIL, 0, 0);
        assert!(matches!(
            c.admin_set_initial_rate(U128(NEAR_USD_MICRO)),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn rotating_the_price_oracle_needs_a_full_access_signature() {
        let mut c = deploy();
        ctx(COUNCIL, 0, 0);
        assert!(matches!(
            c.set_price_oracle(acc(BOB)),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn widening_the_asset_gate_needs_a_full_access_signature() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        assert!(matches!(
            c.add_ft_allowlist(acc("token.testnet")),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn narrowing_the_asset_gate_needs_a_full_access_signature() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        assert!(matches!(
            c.remove_ft_allowlist(acc("token.testnet")),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn opening_a_tla_needs_a_full_access_signature() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        assert!(matches!(
            c.activate_open_tla(acc(TLA)),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn clearing_a_stuck_reclaim_needs_a_full_access_signature() {
        let mut c = deploy();
        ctx(ADMIN, 0, 0);
        assert!(matches!(
            c.admin_clear_reclaim_pending(acc(TLA), "alice".to_string()),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    #[should_panic(expected = "council must not be the registry")]
    fn init_rejects_a_council_that_is_the_registry_itself() {
        ctx(ADMIN, 1, 0);
        let _ = TlaRegistry::new(
            acc(ADMIN),
            acc(HOSEXT),
            U64(GRACE_NS),
            acc(TREASURY),
            acc("registry.testnet"),
            None,
        );
    }
}

#[cfg(test)]
mod deployment_terms {
    use super::*;

    const MINUTE_NS: u64 = 60 * 1_000_000_000;

    fn deploy_with_terms(lease: Option<U64>, grace: u64) -> TlaRegistry {
        ctx(ADMIN, 1, 0);
        TlaRegistry::new(
            acc(ADMIN),
            acc(HOSEXT),
            U64(grace),
            acc(TREASURY),
            acc(COUNCIL),
            lease,
        )
    }

    #[test]
    fn omitting_the_lease_term_takes_the_production_year() {
        let c = deploy_with_terms(None, GRACE_NS);
        assert_eq!(c.lease_term_ns, ONE_YEAR_NS);
    }

    #[test]
    #[should_panic(expected = "lease term too short")]
    fn a_lease_term_under_the_floor_is_refused() {
        let _ = deploy_with_terms(Some(U64(MINUTE_NS - 1)), GRACE_NS);
    }

    #[test]
    #[should_panic(expected = "lease term too long")]
    fn a_lease_term_over_the_ceiling_is_refused() {
        let _ = deploy_with_terms(Some(U64(11 * ONE_YEAR_NS)), GRACE_NS);
    }

    #[test]
    fn a_test_grade_lease_reports_the_deployment_not_ready() {
        let c = deploy_with_terms(Some(U64(MINUTE_NS)), GRACE_NS);
        let readiness = c.deployment_readiness();
        assert!(
            !readiness.production_terms,
            "a one minute lease must not pass as production"
        );
        assert!(
            !readiness.ready,
            "a deployment carrying test-grade terms must never report ready"
        );
    }

    #[test]
    fn a_test_grade_grace_reports_the_deployment_not_ready() {
        let c = deploy_with_terms(None, MINUTE_NS);
        assert!(
            !c.deployment_readiness().production_terms,
            "a one minute grace must not pass as production"
        );
    }

    #[test]
    fn production_terms_pass_when_both_meet_the_real_thresholds() {
        let c = deploy_with_terms(Some(U64(ONE_YEAR_NS)), 30 * 24 * 60 * 60 * 1_000_000_000);
        assert!(
            c.deployment_readiness().production_terms,
            "a year lease and a thirty day grace are production grade"
        );
    }
}

mod valhalla_v2 {
    use super::*;

    #[test]
    fn a_stranger_cannot_sweep_during_the_grace_period() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        ctx(BOB, 1, expires + 1);
        assert!(
            matches!(
                c.reclaim_sweep_near(acc(TLA), "alice".to_string()),
                Err(ContractError::SubAccountNotReclaimable)
            ),
            "a holder who can still renew must not have their balance swept by a stranger"
        );
    }

    #[test]
    fn the_sweep_opens_once_the_name_is_actually_reclaimable() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        ctx(BOB, 1, expires + GRACE_NS + DAY_NS);
        assert!(c.reclaim_sweep_near(acc(TLA), "alice".to_string()).is_ok());
    }

    #[test]
    fn a_delisted_token_can_leave_the_sweepable_set() {
        let mut c = deploy_with_open_tla();
        let token = acc("usdc.testnet");
        ctx(ADMIN, 1, 1);
        c.add_ft_allowlist(token.clone()).unwrap();
        c.remove_ft_allowlist(token.clone()).unwrap();
        ctx(COUNCIL, 1, 1);
        assert!(c.remove_sweepable_token(token.clone()).is_ok());
        ctx(ADMIN, 1, 1);
        assert!(
            c.add_ft_allowlist(token).is_ok(),
            "the gate must be reconfigurable after churn"
        );
    }

    #[test]
    fn a_token_still_on_the_gate_cannot_be_made_unsweepable() {
        let mut c = deploy_with_open_tla();
        let token = acc("usdc.testnet");
        ctx(ADMIN, 1, 1);
        c.add_ft_allowlist(token.clone()).unwrap();
        ctx(COUNCIL, 1, 1);
        assert!(matches!(
            c.remove_sweepable_token(token),
            Err(ContractError::TokenStillAllowlisted)
        ));
    }

    #[test]
    fn the_council_can_shorten_the_lease_term_for_testing() {
        let mut c = deploy_with_open_tla();
        ctx(COUNCIL, 1, 1);
        assert!(c.set_lease_term_ns(U64(300 * 1_000_000_000)).is_ok());
        assert_eq!(c.lease_term_ns, 300 * 1_000_000_000);
    }

    #[test]
    fn a_shortened_term_applies_to_the_next_rental_only() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "before");
        let old = c
            .sub_accounts
            .get(&format!("before.{TLA}"))
            .unwrap()
            .expires_at;
        ctx(COUNCIL, 1, 1);
        c.set_lease_term_ns(U64(60 * 1_000_000_000)).unwrap();
        rent_alice_sub(&mut c, "after");
        let new = c
            .sub_accounts
            .get(&format!("after.{TLA}"))
            .unwrap()
            .expires_at;
        assert!(
            new < old,
            "the new rental must expire far sooner than the old one"
        );
        assert_eq!(
            c.sub_accounts
                .get(&format!("before.{TLA}"))
                .unwrap()
                .expires_at,
            old,
            "an existing name keeps the expiry it was rented with"
        );
    }

    #[test]
    fn only_the_council_may_change_the_lease_term() {
        let mut c = deploy_with_open_tla();
        let before = c.lease_term_ns;
        ctx(ALICE, 1, 1);
        assert!(matches!(
            c.set_lease_term_ns(U64(300 * 1_000_000_000)),
            Err(ContractError::OnlyCouncil)
        ));
        assert_eq!(
            c.lease_term_ns, before,
            "a refused call must not change the term"
        );
    }

    #[test]
    fn a_lease_term_outside_the_bounds_is_refused() {
        let mut c = deploy_with_open_tla();
        ctx(COUNCIL, 1, 1);
        assert!(matches!(
            c.set_lease_term_ns(U64(59 * 1_000_000_000)),
            Err(ContractError::LeaseTermTooShort)
        ));
        assert!(matches!(
            c.set_lease_term_ns(U64(11 * ONE_YEAR_NS)),
            Err(ContractError::LeaseTermTooLong)
        ));
        assert!(c.set_lease_term_ns(U64(60 * 1_000_000_000)).is_ok());
        assert!(c.set_lease_term_ns(U64(10 * ONE_YEAR_NS)).is_ok());
    }

    #[test]
    fn the_owner_index_does_not_keep_an_empty_set_forever() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = format!("alice.{TLA}");
        assert!(c.sub_accounts_by_owner.contains_key(&acc(ALICE)));
        c.sub_account_remove(&key);
        assert!(
            !c.sub_accounts_by_owner.contains_key(&acc(ALICE)),
            "an emptied owner set must leave the outer map, or storage grows with every holder"
        );
    }
}

mod valhalla_v2_more {
    use super::*;

    #[test]
    fn an_orphaned_name_can_be_parked_back_onto_the_re_rent_path() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 1, 1);
        assert!(c.admin_force_park(acc(TLA), "orphan".to_string()).is_ok());
        assert!(
            c.is_name_re_rentable(acc(TLA), "orphan".to_string()),
            "a name whose account exists but whose row was dropped must be re-rentable"
        );
    }

    #[test]
    fn force_park_refuses_a_name_that_is_still_held() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.admin_force_park(acc(TLA), "alice".to_string()),
            Err(ContractError::SubAccountNameTaken)
        ));
    }

    #[test]
    fn force_park_refuses_an_unknown_tla() {
        let mut c = deploy_with_open_tla();
        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.admin_force_park(acc("nosuch.testnet"), "orphan".to_string()),
            Err(ContractError::TlaNotFound)
        ));
    }

    #[test]
    fn a_parked_name_that_still_holds_other_names_cannot_be_re_rented() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "acme");
        let holder: AccountId = format!("acme.{TLA}").parse().unwrap();
        let held = sub_account_key(&acc(TLA), "invoices");
        c.sub_account_insert(
            held,
            SubAccountEntry {
                owner: holder.clone(),
                tla_id: acc(TLA),
                payout_account: holder.clone(),
                rented_at: 1,
                expires_at: GRACE_NS,
                retraction_at: None,
            },
        );
        ctx(ADMIN, 1, 2);
        c.sub_account_remove(&format!("acme.{TLA}"));
        assert!(c.admin_force_park(acc(TLA), "acme".to_string()).is_ok());
        assert!(
            !c.is_name_re_rentable(acc(TLA), "acme".to_string()),
            "the view has to agree with the gate or the checkout offers a name it cannot sell"
        );

        let rent = rent_near_open(&c, "acme");
        ctx(BOB, rent, 2);
        assert!(
            matches!(
                c.rent_sub_account(acc(TLA), "acme".to_string(), None),
                Err(ContractError::NameStillHoldsNames)
            ),
            "re-renting a name that owns other names would hand the buyer everything it holds"
        );
    }

    #[test]
    fn a_parked_name_holding_nothing_re_rents_as_before() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "acme");
        ctx(ADMIN, 1, 2);
        c.sub_account_remove(&format!("acme.{TLA}"));
        assert!(c.admin_force_park(acc(TLA), "acme".to_string()).is_ok());
        assert!(c.is_name_re_rentable(acc(TLA), "acme".to_string()));
    }

    #[test]
    fn a_rotation_that_lands_after_the_name_moved_leaves_it_where_it_is() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = sub_account_key(&acc(TLA), "alice");
        assert!(c.sub_account_reassign(&key, &acc(CAROL), &acc(CAROL)));
        ctx("registry.testnet", 0, 2);
        c.on_sub_account_transferred(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            RotationCause::Transfer,
            Ok(true),
        );
        assert_eq!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .owner,
            acc(CAROL),
            "a transfer approved against an owner who has since moved on must not \
             overwrite whoever holds the name now"
        );
    }

    #[test]
    fn a_recovery_that_lands_after_the_name_moved_leaves_it_where_it_is() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = sub_account_key(&acc(TLA), "alice");
        assert!(c.sub_account_reassign(&key, &acc(CAROL), &acc(CAROL)));
        ctx("registry.testnet", 0, 2);
        c.on_sub_account_recovered(
            acc(TLA),
            "alice".to_string(),
            acc(ALICE),
            acc(BOB),
            Ok(true),
        );
        assert_eq!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .owner,
            acc(CAROL),
            "the wallet and the ledger must agree on who the name left, or a sale \
             racing a recovery splits them"
        );
    }

    #[test]
    fn a_holders_supply_does_not_walk_their_whole_key_set() {
        let mut c = deploy_with_open_tla();
        for name in ["one", "two", "three"] {
            rent_alice_sub(&mut c, name);
        }
        assert_eq!(c.nft_supply_for_owner(acc(ALICE)).0, 3);
        c.sub_account_remove(&format!("two.{TLA}"));
        assert_eq!(
            c.nft_supply_for_owner(acc(ALICE)).0,
            2,
            "the set carries its own length, and removal must be reflected in it"
        );
        c.sub_account_remove(&format!("one.{TLA}"));
        c.sub_account_remove(&format!("three.{TLA}"));
        assert_eq!(
            c.nft_supply_for_owner(acc(ALICE)).0,
            0,
            "an owner whose set was dropped reports nothing, not a stale count"
        );
    }
}

mod valhalla_v2_council_rotation {
    use super::*;

    const NEW_COUNCIL: &str = "council2.testnet";

    #[test]
    fn a_rotation_installs_the_new_council_once_the_delay_has_run() {
        let mut c = deploy();
        let delay = c.upgrade_delay_ns;
        ctx(COUNCIL, 1, 0);
        c.approve_council_rotation(acc(NEW_COUNCIL)).unwrap();
        assert_eq!(c.pending_council(), Some((acc(NEW_COUNCIL), U64(0))));
        ctx(NEW_COUNCIL, 1, delay);
        c.commit_council_rotation().unwrap();
        assert_eq!(c.get_council(), acc(NEW_COUNCIL));
        assert!(c.pending_council().is_none());
    }

    #[test]
    fn the_outgoing_council_cannot_seat_an_account_that_never_signed() {
        let mut c = deploy();
        let delay = c.upgrade_delay_ns;
        ctx(COUNCIL, 1, 0);
        c.approve_council_rotation(acc(NEW_COUNCIL)).unwrap();
        ctx(COUNCIL, 1, delay);
        assert!(
            matches!(
                c.commit_council_rotation(),
                Err(ContractError::OnlyPendingCouncil)
            ),
            "a mistyped council must cost a cancelled rotation, not the contract"
        );
        assert_eq!(c.get_council(), acc(COUNCIL));
    }

    #[test]
    fn a_rotation_cannot_commit_inside_its_delay() {
        let mut c = deploy();
        let delay = c.upgrade_delay_ns;
        ctx(COUNCIL, 1, 0);
        c.approve_council_rotation(acc(NEW_COUNCIL)).unwrap();
        ctx(NEW_COUNCIL, 1, delay - 1);
        assert!(
            matches!(
                c.commit_council_rotation(),
                Err(ContractError::CouncilRotationTooYoung)
            ),
            "the window is what lets anyone watching object before the gate moves"
        );
        assert_eq!(c.get_council(), acc(COUNCIL));
    }

    #[test]
    fn an_approval_can_be_withdrawn_before_it_commits() {
        let mut c = deploy();
        let delay = c.upgrade_delay_ns;
        ctx(COUNCIL, 1, 0);
        c.approve_council_rotation(acc(NEW_COUNCIL)).unwrap();
        ctx(COUNCIL, 1, 1);
        c.cancel_council_rotation().unwrap();
        assert!(c.pending_council().is_none());
        ctx(COUNCIL, 1, delay);
        assert!(matches!(
            c.commit_council_rotation(),
            Err(ContractError::NoCouncilRotationPending)
        ));
        assert_eq!(c.get_council(), acc(COUNCIL));
    }

    #[test]
    fn only_the_council_can_move_the_council() {
        let mut c = deploy();
        ctx(BOB, 1, 0);
        assert!(matches!(
            c.approve_council_rotation(acc(NEW_COUNCIL)),
            Err(ContractError::OnlyCouncil)
        ));
        ctx(COUNCIL, 1, 0);
        c.approve_council_rotation(acc(NEW_COUNCIL)).unwrap();
        ctx(BOB, 1, 1);
        assert!(matches!(
            c.cancel_council_rotation(),
            Err(ContractError::OnlyCouncil)
        ));
        ctx(BOB, 1, c.upgrade_delay_ns);
        assert!(matches!(
            c.commit_council_rotation(),
            Err(ContractError::OnlyPendingCouncil)
        ));
    }

    #[test]
    fn the_rotation_refuses_a_council_that_would_end_the_gate() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        assert!(matches!(
            c.approve_council_rotation(acc(COUNCIL)),
            Err(ContractError::CouncilUnchanged)
        ));
        ctx(COUNCIL, 1, 0);
        assert!(
            matches!(
                c.approve_council_rotation(acc("registry.testnet")),
                Err(ContractError::CouncilIsSelf)
            ),
            "this account ends with no keys, so naming it council closes every gate for good"
        );
    }

    #[test]
    fn moving_the_council_takes_a_full_access_signature() {
        let mut c = deploy();
        ctx(COUNCIL, 0, 0);
        assert!(matches!(
            c.approve_council_rotation(acc(NEW_COUNCIL)),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn the_new_council_gates_what_the_old_one_used_to() {
        let mut c = deploy();
        let delay = c.upgrade_delay_ns;
        ctx(COUNCIL, 1, 0);
        c.approve_council_rotation(acc(NEW_COUNCIL)).unwrap();
        ctx(NEW_COUNCIL, 1, delay);
        c.commit_council_rotation().unwrap();

        ctx(COUNCIL, 1, delay);
        assert!(
            matches!(
                c.register_tla(acc("gone"), TlaType::Open, PremiumCategory::Standard, None),
                Err(ContractError::OnlyCouncil)
            ),
            "the outgoing council must lose the gate it handed over"
        );
        ctx(NEW_COUNCIL, 1, delay);
        assert!(c
            .register_tla(acc("held"), TlaType::Open, PremiumCategory::Standard, None)
            .is_ok());
    }
}

#[test]
fn the_state_layout_is_pinned_to_the_version_that_declares_it() {
    let c = deploy();
    assert_eq!(
        (
            crate::STATE_VERSION,
            near_sdk::borsh::to_vec(&c).unwrap().len()
        ),
        (5, 622),
        "the state shape moved. Bump STATE_VERSION, add a reader in legacy.rs for \
         the shape that is deployed today, and update this fixture. A publish that \
         skips that leaves migrate unable to read what is on the account."
    );
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn stored_value_shapes() -> Vec<(&'static str, String)> {
    let sample = |bytes: Vec<u8>| hex(&bytes);
    vec![
        (
            "TlaEntry",
            sample(
                near_sdk::borsh::to_vec(&TlaEntry {
                    tla_type: TlaType::Business,
                    status: TlaStatus::Active,
                    licensee: Some(acc(ALICE)),
                    premium_category: PremiumCategory::Standard,
                    activated_at: 1,
                    expires_at: 2,
                })
                .unwrap(),
            ),
        ),
        (
            "SubAccountEntry",
            sample(
                near_sdk::borsh::to_vec(&SubAccountEntry {
                    owner: acc(BOB),
                    tla_id: acc(TLA),
                    payout_account: acc(ALICE),
                    rented_at: 1,
                    expires_at: 2,
                    retraction_at: Some(3),
                })
                .unwrap(),
            ),
        ),
        (
            "ParkedEntry",
            sample(
                near_sdk::borsh::to_vec(&ParkedEntry {
                    tla_id: acc(TLA),
                    parked_at: 1,
                })
                .unwrap(),
            ),
        ),
        (
            "PaidOrderState::InFlight",
            sample(
                near_sdk::borsh::to_vec(&PaidOrderState::InFlight {
                    full_name: "a.tla".to_string(),
                    tla_id: acc(TLA),
                    payer: acc(CAROL),
                    started_at: 1,
                })
                .unwrap(),
            ),
        ),
        (
            "PaidOrderState::Settled",
            sample(near_sdk::borsh::to_vec(&PaidOrderState::Settled).unwrap()),
        ),
        (
            "TlaTerms",
            sample(
                near_sdk::borsh::to_vec(&TlaTerms {
                    allocation_fee_usd_micro: Some(U128(1)),
                    tla_rent_usd_micro: Some(U128(2)),
                    sub_fee_usd_micro: Some(U128(3)),
                })
                .unwrap(),
            ),
        ),
        (
            "ActivityRecord",
            sample(
                near_sdk::borsh::to_vec(&ActivityRecord {
                    event: "e".to_string(),
                    account: "a".to_string(),
                    block_height: 1,
                    block_timestamp: 2,
                })
                .unwrap(),
            ),
        ),
    ]
}

#[test]
fn the_shape_of_every_stored_value_is_pinned_too() {
    let pinned = [
        (
            "TlaEntry",
            "0001010d000000616c6963652e746573746e6574020100000000000000\
             0200000000000000",
        ),
        (
            "SubAccountEntry",
            "0b000000626f622e746573746e6574050000006d79746c61\
             0d000000616c6963652e746573746e65740100000000000000\
             0200000000000000010300000000000000",
        ),
        ("ParkedEntry", "050000006d79746c610100000000000000"),
        (
            "PaidOrderState::InFlight",
            "0005000000612e746c61050000006d79746c61\
             0d0000006361726f6c2e746573746e65740100000000000000",
        ),
        ("PaidOrderState::Settled", "01"),
        (
            "TlaTerms",
            "0101000000000000000000000000000000010200000000000000\
             0000000000000000010300000000000000\
             0000000000000000",
        ),
        (
            "ActivityRecord",
            "01000000650100000061010000000000000002000000000000\
             00",
        ),
    ];
    let pinned: Vec<(&str, String)> = pinned
        .iter()
        .map(|(n, h)| (*n, h.chars().filter(|c| !c.is_whitespace()).collect()))
        .collect();
    assert_eq!(
        stored_value_shapes(),
        pinned,
        "a value stored inside a collection changed shape. The contract-struct guard \
         cannot see this, because maps serialise their values lazily and the struct \
         itself only holds their prefixes. Bump STATE_VERSION, add a reader for the \
         shape that is deployed today, and update this fixture."
    );
}

mod valhalla_v1_carryovers {
    use super::*;

    #[test]
    fn a_parked_name_can_still_have_its_balance_swept() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let key = sub_account_key(&acc(TLA), "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        ctx(BOB, 1, expires + GRACE_NS + DAY_NS);
        let _ = c.reclaim_finalize(acc(TLA), "alice".to_string()).unwrap();
        ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
        c.on_reclaim_finalized(acc(TLA), "alice".to_string(), acc(TREASURY), Ok(true));
        assert!(
            c.get_sub_account(acc(TLA), "alice".to_string()).is_none(),
            "the park has to have removed the row for this to prove anything"
        );
        assert!(c.parked_names.contains_key(&key));

        ctx(BOB, 1, expires + GRACE_NS + DAY_NS);
        assert!(
            c.reclaim_sweep_near(acc(TLA), "alice".to_string()).is_ok(),
            "a parked name keeps receiving tokens, so refusing the sweep strands \
             every balance that lands after the park"
        );
    }

    #[test]
    fn a_name_that_is_neither_parked_nor_reclaimable_is_still_refused() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        ctx(BOB, 1, 2);
        assert!(
            matches!(
                c.reclaim_sweep_near(acc(TLA), "alice".to_string()),
                Err(ContractError::SubAccountNotReclaimable)
            ),
            "opening the sweep to parked names must not open it to live ones"
        );
        ctx(BOB, 1, 2);
        assert!(matches!(
            c.reclaim_sweep_near(acc(TLA), "ghost".to_string()),
            Err(ContractError::SubAccountNotFound)
        ));
    }
}

mod tla_lapse_notice {
    use super::*;

    fn lapse_the_tla(c: &mut TlaRegistry) -> u64 {
        let at = c.tlas.get(&acc(TLA)).unwrap().expires_at + GRACE_NS + DAY_NS;
        let key = sub_account_key(&acc(TLA), "alice");
        c.sub_accounts.get_mut(&key).unwrap().expires_at = at + 365 * DAY_NS;
        at
    }

    #[test]
    fn a_lapsed_tla_serves_notice_before_it_takes_a_live_sub() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let at = lapse_the_tla(&mut c);
        let key = sub_account_key(&acc(TLA), "alice");
        assert!(
            c.get_sub_account(acc(TLA), "alice".to_string())
                .unwrap()
                .expires_at
                .0
                > at,
            "the sub's own term must still be live or this proves nothing"
        );

        ctx(BOB, 0, at);
        assert!(
            c.reclaim_finalize(acc(TLA), "alice".to_string()).is_ok(),
            "the first call serves the notice rather than rotating"
        );
        assert!(
            c.sub_accounts.get(&key).unwrap().retraction_at.is_some(),
            "the notice has to be recorded or the wait is unenforceable"
        );

        ctx(BOB, 0, at + 1);
        assert!(
            matches!(
                c.reclaim_finalize(acc(TLA), "alice".to_string()),
                Err(ContractError::RetractionPending)
            ),
            "a lapsed parent must not take a paid-up name before the notice runs"
        );
    }

    #[test]
    fn the_reclaim_completes_once_the_notice_has_run() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let at = lapse_the_tla(&mut c);
        ctx(BOB, 0, at);
        let _ = c.reclaim_finalize(acc(TLA), "alice".to_string()).unwrap();

        let notice = c.get_fee_config().retraction_notice_ns.0;
        ctx(BOB, 0, at + notice);
        assert!(c.reclaim_finalize(acc(TLA), "alice".to_string()).is_ok());
    }

    #[test]
    fn an_expired_sub_is_reclaimed_without_any_extra_wait() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "alice");
        let expires = c
            .get_sub_account(acc(TLA), "alice".to_string())
            .unwrap()
            .expires_at
            .0;
        ctx(BOB, 0, expires + GRACE_NS + DAY_NS);
        assert!(c.reclaim_finalize(acc(TLA), "alice".to_string()).is_ok());
        let key = sub_account_key(&acc(TLA), "alice");
        assert!(
            c.reclaim_pending.contains_key(&key),
            "a term that ran on its own must rotate on the first call"
        );
    }
}

mod cursor_pages {
    use super::*;

    #[test]
    fn a_cursor_page_walks_a_holders_names_without_an_offset() {
        let mut c = deploy_with_open_tla();
        for n in ["one", "two", "three", "four", "five"] {
            rent_alice_sub(&mut c, n);
        }
        let first = c
            .nft_tokens_for_owner_page(acc(ALICE), None, Some(2))
            .unwrap();
        assert_eq!(first.tokens.len(), 2);
        let cursor = first.next.clone().expect("more names remain");

        let second = c
            .nft_tokens_for_owner_page(acc(ALICE), Some(cursor), Some(2))
            .unwrap();
        assert_eq!(second.tokens.len(), 2);
        let third = c
            .nft_tokens_for_owner_page(acc(ALICE), second.next.clone(), Some(2))
            .unwrap();
        assert_eq!(third.tokens.len(), 1);
        assert!(third.next.is_none(), "the last page ends the walk");

        let mut seen: Vec<String> = first
            .tokens
            .iter()
            .chain(second.tokens.iter())
            .chain(third.tokens.iter())
            .map(|t| t.token_id.clone())
            .collect();
        seen.sort();
        seen.dedup();
        assert_eq!(
            seen.len(),
            5,
            "every name appears exactly once across pages"
        );
    }

    #[test]
    fn a_cursor_page_for_a_holder_with_nothing_is_empty() {
        let c = deploy_with_open_tla();
        let page = c.nft_tokens_for_owner_page(acc(BOB), None, None).unwrap();
        assert!(page.tokens.is_empty());
        assert!(page.next.is_none());
    }

    #[test]
    fn a_cursor_naming_a_name_the_holder_no_longer_has_is_refused() {
        let mut c = deploy_with_open_tla();
        for n in ["one", "two", "three"] {
            rent_alice_sub(&mut c, n);
        }
        let first = c
            .nft_tokens_for_owner_page(acc(ALICE), None, Some(1))
            .unwrap();
        let cursor = first.next.clone().expect("more names remain");
        assert!(
            c.nft_tokens_for_owner_page(acc(ALICE), Some(cursor.clone()), Some(1))
                .is_ok(),
            "the cursor resumes while the name it points at is still held"
        );

        c.sub_account_remove(&cursor);

        let Err(err) = c.nft_tokens_for_owner_page(acc(ALICE), Some(cursor), Some(1)) else {
            panic!("a cursor that no longer exists cannot silently end the walk");
        };
        assert!(matches!(err, ContractError::UnknownCursor));
    }

    #[test]
    fn a_holder_who_moved_every_name_gets_a_refusal_rather_than_an_empty_last_page() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "one");
        rent_alice_sub(&mut c, "two");
        let first = c
            .nft_tokens_for_owner_page(acc(ALICE), None, Some(1))
            .unwrap();
        let cursor = first.next.clone().expect("more names remain");

        c.sub_account_remove(&sub_account_key(&acc(TLA), "one"));
        c.sub_account_remove(&sub_account_key(&acc(TLA), "two"));

        let Err(err) = c.nft_tokens_for_owner_page(acc(ALICE), Some(cursor), Some(1)) else {
            panic!("an emptied index must not read as the end of the walk");
        };
        assert!(matches!(err, ContractError::UnknownCursor));
    }

    #[test]
    fn a_zero_limit_still_hands_back_a_usable_cursor() {
        let mut c = deploy_with_open_tla();
        rent_alice_sub(&mut c, "one");
        rent_alice_sub(&mut c, "two");
        let page = c
            .nft_tokens_for_owner_page(acc(ALICE), None, Some(0))
            .unwrap();
        assert_eq!(
            page.tokens.len(),
            1,
            "a page of nothing with no cursor cannot say whether more remain"
        );
        assert!(page.next.is_some(), "the walk can still be resumed");
    }
}

mod notice_floor_interop {
    use super::*;

    #[test]
    fn the_pushed_notice_clears_the_wallet_floor_at_the_shortest_legal_setting() {
        let mut c = deploy_with_open_tla();
        let mut fees = c.get_fee_config();
        fees.retraction_notice_ns = U64(24 * 60 * 60 * 1_000_000_000);
        ctx(COUNCIL, 1, 1);
        c.update_fee_config(fees).unwrap();

        let notice = c.get_fee_config().retraction_notice_ns.0;
        assert!(
            notice > hos_common::MIN_LEASE_RETRACT_NOTICE_NS,
            "the registry pushes now plus {notice} and the wallet re-checks it a block \
             later against its own floor, so the two must not be equal"
        );
    }
}

mod launch_batches {
    use super::*;
    use crate::admin::MAX_TLA_BATCH;

    fn register_open(c: &mut TlaRegistry, ids: Vec<AccountId>) -> Result<(), ContractError> {
        c.register_tlas(ids, TlaType::Open, PremiumCategory::Standard, None)
    }

    fn names(count: usize) -> Vec<AccountId> {
        (0..count).map(|i| acc(&format!("seed{i}"))).collect()
    }

    const LAUNCH_SEED: usize = 3_900;
    const PER_CALL: usize = MAX_TLA_BATCH;
    const CALLS_PER_PASS: usize = LAUNCH_SEED.div_ceil(PER_CALL);

    fn seed_name(index: usize) -> String {
        format!("seed{index}")
    }

    #[test]
    fn the_whole_launch_seed_registers_and_opens_at_the_measured_batch_size() {
        let mut c = deploy();
        let all: Vec<String> = (0..LAUNCH_SEED).map(seed_name).collect();

        let mut register_calls = 0;
        for chunk in all.chunks(PER_CALL) {
            ctx(COUNCIL, 1, 0);
            register_open(&mut c, chunk.iter().map(|n| acc(n)).collect()).unwrap();
            register_calls += 1;
        }

        let mut open_calls = 0;
        for chunk in all.chunks(PER_CALL) {
            ctx(ADMIN, 1, 1);
            c.activate_open_tlas(chunk.iter().map(|n| acc(n)).collect())
                .unwrap();
            open_calls += 1;
        }

        assert_eq!(
            (register_calls, open_calls),
            (CALLS_PER_PASS, CALLS_PER_PASS)
        );
        for index in [0, LAUNCH_SEED / 2, LAUNCH_SEED - 1] {
            let view = c.get_tla(acc(&seed_name(index))).unwrap();
            assert!(
                matches!(view.lifecycle, LifecycleStatus::Active),
                "{} did not come out open",
                seed_name(index)
            );
        }
        assert!(c.is_name_available(acc(&seed_name(LAUNCH_SEED - 1)), "alice".to_string()));
    }

    #[test]
    fn a_payload_that_would_overrun_the_log_budget_is_refused() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        let too_many: Vec<AccountId> = (0..1_900).map(|i| acc(&seed_name(i))).collect();
        assert!(matches!(
            register_open(&mut c, too_many),
            Err(ContractError::BatchTooLarge)
        ));
        assert!(c.get_tla(acc(&seed_name(0))).is_none());
    }

    #[test]
    fn the_council_registers_a_whole_batch_in_one_call() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        register_open(&mut c, names(PER_CALL)).unwrap();
        assert!(c.get_tla(acc("seed0")).is_some());
        assert!(c.get_tla(acc(&format!("seed{}", PER_CALL - 1))).is_some());
    }

    #[test]
    fn an_operations_admin_cannot_register_a_batch() {
        ctx(ADMIN, 1, 0);
        let mut c = TlaRegistry::new(
            acc(ADMIN),
            acc(HOSEXT),
            U64(GRACE_NS),
            acc(TREASURY),
            acc(OTHER_COUNCIL),
            None,
        );
        ctx(ADMIN, 1, 0);
        assert!(matches!(
            register_open(&mut c, names(2)),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn one_bad_name_rolls_the_whole_batch_back() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        register_open(&mut c, vec![acc("taken")]).unwrap();

        ctx(COUNCIL, 1, 0);
        assert!(matches!(
            register_open(&mut c, vec![acc("fresh"), acc("taken")]),
            Err(ContractError::TlaAlreadyRegistered)
        ));
        assert!(
            c.get_tla(acc("fresh")).is_none(),
            "a batch that fails part way must leave nothing behind, or a re-run of the \
             same proposal payload trips on the names the failed run already wrote"
        );
    }

    #[test]
    fn a_batch_past_the_cap_is_refused_before_it_runs_out_of_gas() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        assert!(matches!(
            register_open(&mut c, names(MAX_TLA_BATCH + 1)),
            Err(ContractError::BatchTooLarge)
        ));
        assert!(c.get_tla(acc("seed0")).is_none());
    }

    #[test]
    fn the_same_name_twice_in_one_payload_is_caught() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        assert!(
            matches!(
                register_open(&mut c, vec![acc("twice"), acc("other"), acc("twice")]),
                Err(ContractError::DuplicateInBatch)
            ),
            "a generated seed list of thousands is exactly where a repeat hides"
        );
        assert!(c.get_tla(acc("other")).is_none());
    }

    #[test]
    fn an_empty_batch_is_refused_rather_than_passing_silently() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        assert!(matches!(
            register_open(&mut c, vec![]),
            Err(ContractError::EmptyBatch)
        ));
    }

    #[test]
    fn a_batch_registration_still_needs_one_yocto() {
        let mut c = deploy();
        ctx(COUNCIL, 0, 0);
        assert!(register_open(&mut c, names(1)).is_err());
    }

    #[test]
    fn admin_opens_a_batch_and_the_names_become_rentable() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        register_open(&mut c, names(3)).unwrap();

        ctx(ADMIN, 1, 1);
        c.activate_open_tlas(vec![acc("seed0"), acc("seed1"), acc("seed2")])
            .unwrap();
        for i in 0..3 {
            let view = c.get_tla(acc(&format!("seed{i}"))).unwrap();
            assert!(matches!(view.lifecycle, LifecycleStatus::Active));
        }
        assert!(c.is_name_available(acc("seed0"), "alice".to_string()));
    }

    #[test]
    fn opening_a_batch_rolls_back_when_one_was_never_registered() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        register_open(&mut c, names(1)).unwrap();

        ctx(ADMIN, 1, 1);
        assert!(matches!(
            c.activate_open_tlas(vec![acc("seed0"), acc("missing")]),
            Err(ContractError::TlaNotFound)
        ));
        let view = c.get_tla(acc("seed0")).unwrap();
        assert!(!matches!(view.lifecycle, LifecycleStatus::Active));
    }

    #[test]
    fn the_single_call_paths_still_work_alongside_the_batches() {
        let mut c = deploy();
        ctx(COUNCIL, 1, 0);
        c.register_tla(acc("solo"), TlaType::Open, PremiumCategory::Standard, None)
            .unwrap();
        ctx(ADMIN, 1, 1);
        c.activate_open_tla(acc("solo")).unwrap();
        let view = c.get_tla(acc("solo")).unwrap();
        assert!(matches!(view.lifecycle, LifecycleStatus::Active));
    }
}
