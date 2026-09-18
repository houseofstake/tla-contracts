use super::*;
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::testing_env;
use std::str::FromStr;

const ADMIN: &str = "hos.testnet";
const REGISTRY: &str = "tla-registry.testnet";
const RECOVERY: &str = "mpc-recovery.testnet";
const WALLET: &str = "alice.tla.testnet";
const TOKEN: &str = "token.testnet";
const DEST: &str = "treasury.testnet";
const BUYER: &str = "buyer.testnet";
const COUNCIL: &str = "council.testnet";
fn acc(s: &str) -> AccountId {
    AccountId::from_str(s).unwrap()
}

fn ctx(predecessor: &str, deposit: u128) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(acc("hos-extension.testnet"))
        .predecessor_account_id(acc(predecessor))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .account_balance(NearToken::from_near(10))
        .build());
}

fn ctx_callback(result: near_sdk::PromiseResult) {
    testing_env!(
        VMContextBuilder::new()
            .current_account_id(acc("hos-extension.testnet"))
            .predecessor_account_id(acc("hos-extension.testnet"))
            .build(),
        near_sdk::test_vm_config(),
        near_sdk::RuntimeFeesConfig::test(),
        Default::default(),
        vec![result],
    );
}

fn deploy() -> HosExtension {
    ctx(ADMIN, 0);
    HosExtension::new(
        acc(ADMIN),
        acc(REGISTRY),
        acc(RECOVERY),
        acc(DEST),
        acc(COUNCIL),
    )
}

fn sweep_deposit() -> u128 {
    MIN_SWEEP_ATTACHED.as_yoctonear()
}

#[test]
fn registry_sells_via_force_transfer() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(c
        .force_transfer(
            acc(WALLET),
            Some(acc(BUYER)),
            RotationCause::Sale,
            Some(acc(BUYER))
        )
        .is_ok());
}

#[test]
fn registry_parks_via_force_transfer() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(c
        .force_transfer(acc(WALLET), None, RotationCause::Reclaim, None)
        .is_ok());
}

#[test]
fn park_with_new_owner_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.force_transfer(acc(WALLET), Some(acc(BUYER)), RotationCause::Reclaim, None),
        Err(ContractError::ParkTakesNoOwner)
    ));
}

#[test]
fn transfer_without_new_owner_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.force_transfer(acc(WALLET), None, RotationCause::Sale, Some(acc(BUYER))),
        Err(ContractError::TransferNeedsOwner)
    ));
}

#[test]
fn re_rent_cause_rejected_on_force_transfer() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.force_transfer(
            acc(WALLET),
            Some(acc(BUYER)),
            RotationCause::ReRent,
            Some(acc(BUYER))
        ),
        Err(ContractError::ReRentNeedsOwnCall)
    ));
}

#[test]
fn registry_re_rents_with_its_own_payout() {
    let mut c = deploy();
    ctx(REGISTRY, 1);
    assert!(c
        .re_rent(acc(WALLET), acc(BUYER), acc(DEST), U64(1))
        .is_ok());
}

#[test]
fn a_failed_near_sweep_is_recorded_so_it_can_be_run_again() {
    let mut c = deploy();
    ctx_callback(near_sdk::PromiseResult::Failed);
    assert!(!c.after_near_sweep(acc(WALLET), Err(near_sdk::PromiseError::Failed)));
    assert_eq!(c.pending_sweep_count(), 1);
    assert_eq!(
        c.pending_sweeps(None, None),
        vec![PendingSweep {
            wallet: acc(WALLET),
            ft: None
        }],
        "a sweep nobody records is a payout nobody ever retries"
    );

    ctx(REGISTRY, 1);
    assert!(c.retry_sweep(acc(WALLET), None).is_ok());

    ctx_callback(near_sdk::PromiseResult::Successful(
        near_sdk::serde_json::to_vec(&true).unwrap(),
    ));
    assert!(c.after_near_sweep(acc(WALLET), Ok(true)));
    assert_eq!(
        c.pending_sweep_count(),
        0,
        "a sweep that finally lands must stop asking to be retried"
    );
}

#[test]
fn a_failed_token_sweep_is_recorded_against_its_own_token() {
    let mut c = deploy();
    ctx_callback(near_sdk::PromiseResult::Failed);
    c.after_sweep_settled(
        acc(WALLET),
        acc(TOKEN),
        acc(DEST),
        U128(5),
        Err(near_sdk::PromiseError::Failed),
    );
    assert_eq!(
        c.pending_sweeps(None, None),
        vec![PendingSweep {
            wallet: acc(WALLET),
            ft: Some(acc(TOKEN))
        }],
        "a token sweep must be retryable for the token that failed, not the wallet at large"
    );
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.retry_sweep(acc(WALLET), None),
        Err(ContractError::NoPendingSweep)
    ));
}

#[test]
fn a_sweep_that_never_failed_cannot_be_retried() {
    let mut c = deploy();
    ctx(REGISTRY, 1);
    assert!(matches!(
        c.retry_sweep(acc(WALLET), None),
        Err(ContractError::NoPendingSweep)
    ));
}

#[test]
fn only_the_registry_can_retry_a_sweep() {
    let mut c = deploy();
    ctx_callback(near_sdk::PromiseResult::Failed);
    c.after_near_sweep(acc(WALLET), Err(near_sdk::PromiseError::Failed));
    ctx(BUYER, 1);
    assert!(matches!(
        c.retry_sweep(acc(WALLET), None),
        Err(ContractError::OnlyRegistry)
    ));
}

#[test]
fn a_settled_re_rent_clears_the_recovery_policy_the_last_holder_armed() {
    let mut c = deploy();
    ctx_callback(near_sdk::PromiseResult::Successful(
        near_sdk::serde_json::to_vec(&true).unwrap(),
    ));
    assert!(
        c.after_re_rent(acc(WALLET), Ok(true)),
        "a settled re-rent must report the rotation to its caller"
    );
    assert!(
        near_sdk::test_utils::get_logs()
            .iter()
            .any(|l| l.contains("force_transfer_completed")),
        "the re-rent must run the same post-rotation path a force transfer does, or the \
         previous holder keeps an armed recovery policy over the new renter's name"
    );
}

#[test]
fn a_refused_re_rent_leaves_the_recovery_policy_alone() {
    let mut c = deploy();
    ctx_callback(near_sdk::PromiseResult::Failed);
    assert!(!c.after_re_rent(acc(WALLET), Ok(false)));
    assert!(near_sdk::test_utils::get_logs()
        .iter()
        .any(|l| l.contains("force_transfer_voided")));
}

#[test]
fn non_registry_cannot_re_rent() {
    let mut c = deploy();
    ctx(ADMIN, 1);
    assert!(matches!(
        c.re_rent(acc(WALLET), acc(BUYER), acc(DEST), U64(1)),
        Err(ContractError::OnlyRegistry)
    ));
}

#[test]
fn re_rent_without_one_yocto_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.re_rent(acc(WALLET), acc(BUYER), acc(DEST), U64(1)),
        Err(ContractError::RequiresOneYocto)
    ));
}

#[test]
fn non_registry_cannot_force_transfer() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    assert!(matches!(
        c.force_transfer(
            acc(WALLET),
            Some(acc(BUYER)),
            RotationCause::Sale,
            Some(acc(BUYER))
        ),
        Err(ContractError::OnlyRegistry)
    ));
}

#[test]
fn paused_blocks_force_transfer() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    c.pause().unwrap();
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.force_transfer(
            acc(WALLET),
            Some(acc(BUYER)),
            RotationCause::Sale,
            Some(acc(BUYER))
        ),
        Err(ContractError::Paused)
    ));
}

#[test]
fn registry_pushes_lease() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(c
        .push_lease(acc(WALLET), U64(123), OperatingState::Listed)
        .is_ok());
}

#[test]
fn non_registry_cannot_push_lease() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    assert!(matches!(
        c.push_lease(acc(WALLET), U64(123), OperatingState::Listed),
        Err(ContractError::OnlyRegistry)
    ));
}

#[test]
fn registry_sweeps_ft_with_correct_deposit() {
    let mut c = deploy();
    ctx(REGISTRY, sweep_deposit());
    assert!(c.sweep_ft(acc(WALLET), acc(TOKEN), acc(BUYER)).is_ok());
}

#[test]
fn sweep_rejects_wrong_deposit() {
    let mut c = deploy();
    ctx(REGISTRY, sweep_deposit() - 1);
    assert!(matches!(
        c.sweep_ft(acc(WALLET), acc(TOKEN), acc(BUYER)),
        Err(ContractError::InsufficientDeposit)
    ));
}

#[test]
fn non_registry_cannot_sweep() {
    let mut c = deploy();
    ctx(ADMIN, sweep_deposit());
    assert!(matches!(
        c.sweep_ft(acc(WALLET), acc(TOKEN), acc(BUYER)),
        Err(ContractError::OnlyRegistry)
    ));
}

#[test]
fn admin_can_skim_within_available_balance() {
    let mut c = deploy();
    ctx(ADMIN, 1);
    assert!(c.skim(U128(1)).is_ok());
}

#[test]
fn non_admin_cannot_skim() {
    let mut c = deploy();
    ctx(REGISTRY, 1);
    assert!(matches!(c.skim(U128(1)), Err(ContractError::OnlyAdmin)));
}

#[test]
fn a_restricted_key_cannot_skim() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    assert!(matches!(
        c.skim(U128(1)),
        Err(ContractError::RequiresOneYocto)
    ));
}

fn ctx_at(predecessor: &str, deposit: u128, ts: u64) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(acc("hos-extension.testnet"))
        .predecessor_account_id(acc(predecessor))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .account_balance(NearToken::from_near(10))
        .block_timestamp(ts)
        .build());
}

fn approve(c: &mut HosExtension, code: &[u8], ts: u64) {
    ctx_at(COUNCIL, 1, ts);
    c.approve_upgrade(Base58CryptoHash::from(env::sha256_array(code)))
        .unwrap();
}

#[test]
fn admin_upgrade_returns_promise_once_the_delay_has_run() {
    let mut c = deploy();
    let code = vec![1, 2, 3];
    approve(&mut c, &code, 0);
    ctx_at(ADMIN, 1, UPGRADE_DELAY_NS);
    assert!(c.upgrade(Base64VecU8(code)).is_ok());
}

#[test]
fn upgrade_rejects_code_that_was_never_approved() {
    let mut c = deploy();
    ctx_at(ADMIN, 1, UPGRADE_DELAY_NS);
    assert!(matches!(
        c.upgrade(Base64VecU8(vec![1, 2, 3])),
        Err(ContractError::NoApprovedHash)
    ));
}

#[test]
fn upgrade_rejects_code_that_does_not_match_the_approved_hash() {
    let mut c = deploy();
    approve(&mut c, &[1, 2, 3], 0);
    ctx_at(ADMIN, 1, UPGRADE_DELAY_NS);
    assert!(matches!(
        c.upgrade(Base64VecU8(vec![9, 9, 9])),
        Err(ContractError::HashMismatch)
    ));
}

#[test]
fn upgrade_rejects_an_approval_that_has_not_aged() {
    let mut c = deploy();
    let code = vec![1, 2, 3];
    approve(&mut c, &code, 0);
    ctx_at(ADMIN, 1, UPGRADE_DELAY_NS - 1);
    assert!(matches!(
        c.upgrade(Base64VecU8(code)),
        Err(ContractError::ApprovalTooYoung)
    ));
}

#[test]
fn a_landed_upgrade_clears_the_approval_so_it_cannot_be_replayed() {
    let mut c = deploy();
    let code = vec![1, 2, 3];
    approve(&mut c, &code, 0);
    ctx_at(ADMIN, 1, UPGRADE_DELAY_NS);
    assert!(c.upgrade(Base64VecU8(code.clone())).is_ok());
    ctx_at(ADMIN, 1, UPGRADE_DELAY_NS * 2);
    assert!(matches!(
        c.upgrade(Base64VecU8(code)),
        Err(ContractError::NoApprovedHash)
    ));
}

#[test]
fn skim_pays_whichever_treasury_the_contract_currently_holds() {
    let mut c = deploy();
    ctx(ADMIN, 1);
    assert!(c.skim(U128(1)).is_ok());
    assert_eq!(c.treasury, acc(DEST));
}

#[test]
fn upgrade_rejects_empty_code() {
    let mut c = deploy();
    ctx(ADMIN, 1);
    assert!(matches!(
        c.upgrade(Base64VecU8(Vec::new())),
        Err(ContractError::EmptyCode)
    ));
}

#[test]
fn non_admin_cannot_upgrade() {
    let mut c = deploy();
    ctx(REGISTRY, 1);
    assert!(matches!(
        c.upgrade(Base64VecU8(vec![1])),
        Err(ContractError::OnlyAdmin)
    ));
}

#[test]
fn admin_management_keeps_at_least_one() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    assert!(matches!(
        c.remove_admin(acc(ADMIN)),
        Err(ContractError::CannotRemoveLastAdmin)
    ));
    c.add_admin(acc("second.testnet")).unwrap();
    c.remove_admin(acc(ADMIN)).unwrap();
    assert_eq!(c.get_admins(), vec![acc("second.testnet")]);
}

#[test]
fn privileged_methods_reject_a_restricted_access_key() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    assert!(matches!(
        c.add_admin(acc("second.testnet")),
        Err(ContractError::RequiresOneYocto)
    ));
    assert!(matches!(
        c.remove_admin(acc(ADMIN)),
        Err(ContractError::RequiresOneYocto)
    ));
    assert!(matches!(
        c.upgrade(Base64VecU8(vec![1])),
        Err(ContractError::RequiresOneYocto)
    ));
}

#[test]
fn after_force_swap_notifies_recovery_on_success() {
    let mut c = deploy();
    ctx("hos-extension.testnet", 0);
    assert!(c.after_force_swap(acc(WALLET), Ok(())));
}

#[test]
fn after_force_swap_voided_when_the_wallet_call_fails() {
    let mut c = deploy();
    ctx("hos-extension.testnet", 0);
    assert!(!c.after_force_swap(acc(WALLET), Err(PromiseError::Failed)));
}

#[test]
fn config_views() {
    let c = deploy();
    assert_eq!(c.get_registry(), acc(REGISTRY));
    assert_eq!(c.get_recovery(), acc(RECOVERY));
    assert_eq!(c.get_version(), CONTRACT_VERSION);
    assert!(!c.is_paused());
}

#[test]
fn an_admin_cannot_add_another_admin() {
    let mut c = deploy();
    ctx(ADMIN, 1);
    assert!(matches!(
        c.add_admin(acc("second.testnet")),
        Err(ContractError::OnlyCouncil)
    ));
}

#[test]
fn only_the_registry_can_move_a_payout_or_sweep_a_wallet() {
    let mut c = deploy();
    ctx(ADMIN, 1);
    assert!(
        matches!(
            c.set_payout(acc(WALLET), acc(DEST), acc(BUYER)),
            Err(ContractError::OnlyRegistry)
        ),
        "an admin reaching a wallet directly would bypass every check the registry performs first"
    );
    ctx(ADMIN, 1);
    assert!(matches!(
        c.sweep_near(acc(WALLET)),
        Err(ContractError::OnlyRegistry)
    ));
    ctx(COUNCIL, 1);
    assert!(matches!(
        c.sweep_near(acc(WALLET)),
        Err(ContractError::OnlyRegistry)
    ));
}

#[test]
fn only_an_admin_can_lift_a_pause() {
    let mut c = deploy();
    ctx(ADMIN, 1);
    c.pause().unwrap();
    ctx("mallory.testnet", 1);
    assert!(matches!(c.unpause(), Err(ContractError::OnlyAdmin)));
    assert!(c.is_paused());
    ctx(ADMIN, 1);
    c.unpause().unwrap();
    assert!(!c.is_paused());
}

#[test]
fn an_admin_cannot_approve_an_upgrade() {
    let mut c = deploy();
    ctx_at(ADMIN, 1, 0);
    assert!(matches!(
        c.approve_upgrade(Base58CryptoHash::from(env::sha256_array([1, 2, 3]))),
        Err(ContractError::OnlyCouncil)
    ));
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
        ctx(ADMIN, 0);
        env::state_write(&c);
        HosExtension::migrate();
    }
}
#[test]
fn state_already_current_survives_a_same_shape_redeploy() {
    let current = deploy();
    let registry = current.registry.clone();
    env::state_write(&current);
    assert_eq!(HosExtension::migrate().registry, registry);
}

#[test]
fn a_reset_the_recovery_contract_deferred_stays_pending() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    c.after_recovery_reset(acc(WALLET), Ok(false));
    assert_eq!(
        c.pending_recovery_resets(None, None),
        vec![acc(WALLET)],
        "a deferred reset reports success on the wire, so treating any Ok as done would drop \
         the one case where the previous owner keeps their recovery policy"
    );
}

#[test]
fn a_completed_reset_clears_the_pending_entry() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    c.after_recovery_reset(acc(WALLET), Ok(false));
    c.after_recovery_reset(acc(WALLET), Ok(true));
    assert!(c.pending_recovery_resets(None, None).is_empty());
}

#[test]
fn a_failed_reset_stays_pending() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    c.after_recovery_reset(acc(WALLET), Err(near_sdk::PromiseError::Failed));
    assert_eq!(c.pending_recovery_resets(None, None), vec![acc(WALLET)]);
}

#[test]
fn a_sweep_that_could_not_read_the_payout_refunds_the_caller() {
    let mut c = deploy();
    ctx(REGISTRY, sweep_deposit());
    let _ = c.after_payout_for_sweep(
        acc(WALLET),
        acc(TOKEN),
        acc(BUYER),
        Err(near_sdk::PromiseError::Failed),
    );
    assert_eq!(
        refunds(),
        vec![(acc(BUYER), sweep_deposit())],
        "the caller paid the storage deposit up front, so a sweep that never dispatched owes it \
         back to them rather than keeping it here"
    );
}

#[test]
fn a_sweep_the_wallet_refused_is_not_reported_as_dispatched() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(!c.after_sweep_settled(
        acc(WALLET),
        acc(TOKEN),
        acc(DEST),
        U128(5),
        Err(near_sdk::PromiseError::Failed),
    ));
    let logs = near_sdk::test_utils::get_logs();
    assert!(logs.iter().any(|l| l.contains("sweep_failed")));
    assert!(
        !logs.iter().any(|l| l.contains("sweep_dispatched")),
        "the tokens never moved, so the feed must not say they did"
    );
}

fn refunds() -> Vec<(AccountId, u128)> {
    near_sdk::test_utils::get_created_receipts()
        .into_iter()
        .flat_map(|receipt| {
            let to = receipt.receiver_id.clone();
            receipt
                .actions
                .into_iter()
                .filter_map(move |action| match action {
                    near_sdk::mock::MockAction::Transfer { deposit, .. } => {
                        Some((to.clone(), deposit.as_yoctonear()))
                    }
                    _ => None,
                })
        })
        .collect()
}

#[test]
fn the_pending_reset_backlog_stays_readable_once_it_is_large() {
    let mut c = deploy();
    for i in 0..600 {
        if i % 90 == 0 {
            ctx(ADMIN, 0);
        }
        c.after_recovery_reset(acc(&format!("w{i}.tla.testnet")), Ok(false));
    }
    assert_eq!(c.pending_recovery_reset_count(), 600);
    let first = c.pending_recovery_resets(None, None);
    assert_eq!(
        first.len(),
        500,
        "the page is capped, so a backlog can never outgrow the call that has to read it"
    );
    let rest = c.pending_recovery_resets(Some(500), None);
    assert_eq!(rest.len(), 100, "the tail is reachable by offset");
    let past_the_end = c.pending_recovery_resets(Some(u64::from(u32::MAX) + 1), None);
    assert!(
        past_the_end.is_empty(),
        "an offset beyond a 32 bit usize must run off the end, not wrap to the first page"
    );
}

#[test]
fn the_admin_set_is_bounded() {
    let mut c = deploy();
    while c.get_admins().len() < 32 {
        let next = format!("admin{}.testnet", c.get_admins().len());
        ctx(COUNCIL, 1);
        c.add_admin(acc(&next)).unwrap();
    }
    ctx(COUNCIL, 1);
    assert!(matches!(
        c.add_admin(acc("one-too-many.testnet")),
        Err(ContractError::AdminSetFull)
    ));
    ctx(COUNCIL, 1);
    c.remove_admin(acc("admin31.testnet")).unwrap();
    ctx(COUNCIL, 1);
    assert!(
        c.add_admin(acc("one-too-many.testnet")).is_ok(),
        "the cap bounds the set, it does not close the seat permanently"
    );
}

#[test]
fn a_seal_that_did_not_remove_the_key_is_not_reported_as_sealed() {
    let mut c = deploy();
    ctx_callback(near_sdk::PromiseResult::Failed);
    assert!(!c.after_seal("ed25519:key".to_string(), acc(COUNCIL)));
    let logs = near_sdk::test_utils::get_logs();
    assert!(
        logs.iter().any(|l| l.contains("seal_failed")),
        "expected a failure event, got {logs:?}"
    );
    assert!(
        !logs.iter().any(|l| l.contains(r#""event":"sealed""#)),
        "the launch gate reads this log, so a key that survived must never read as sealed"
    );
}

#[test]
fn a_seal_that_removed_the_key_is_reported_as_sealed() {
    let mut c = deploy();
    ctx_callback(near_sdk::PromiseResult::Successful(vec![]));
    assert!(c.after_seal("ed25519:key".to_string(), acc(COUNCIL)));
    assert!(near_sdk::test_utils::get_logs()
        .iter()
        .any(|l| l.contains(r#""event":"sealed""#)));
}

#[test]
fn a_skim_that_never_landed_is_not_reported_as_a_payment() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    ctx_callback(near_sdk::PromiseResult::Failed);
    assert!(
        !c.after_skim(U128(7), acc(DEST), acc(ADMIN)),
        "the treasury never received it, so the event must say so"
    );
    let logs = near_sdk::test_utils::get_logs();
    assert!(
        logs.iter().any(|l| l.contains("skim_failed")),
        "expected a failure event, got {logs:?}"
    );
    assert!(
        !logs.iter().any(|l| l.contains("balance_skimmed")),
        "a failed transfer must never log the completion event"
    );
}

#[test]
fn retry_refuses_a_wallet_that_is_not_pending() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    assert!(matches!(
        c.retry_recovery_reset(acc(WALLET)),
        Err(ContractError::NoPendingReset)
    ));
}

fn a_key() -> near_sdk::PublicKey {
    std::str::FromStr::from_str("ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847").unwrap()
}

#[test]
fn the_council_can_seal_the_extension_once_the_upgrade_path_is_proven() {
    let mut c = deploy();
    c.upgrade_proven = true;
    ctx(COUNCIL, 1);
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
        "the launch gate turns on this account ending with no key, so the seal has to schedule \
         the removal rather than only report one"
    );
}

#[test]
fn sealing_is_refused_until_an_upgrade_has_run_on_this_account() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    assert!(
        matches!(c.seal(a_key()), Err(ContractError::UpgradeNotProven)),
        "removing the key before the upgrade path is proven leaves an account with no \
         way to change its code and no way back"
    );
}

#[test]
fn an_admin_alone_cannot_remove_the_key() {
    let mut c = deploy();
    ctx(ADMIN, 1);
    assert!(matches!(c.seal(a_key()), Err(ContractError::OnlyCouncil)));
}

#[test]
fn sealing_the_extension_needs_a_full_access_signature() {
    let mut c = deploy();
    ctx(COUNCIL, 0);
    assert!(matches!(
        c.seal(a_key()),
        Err(ContractError::RequiresOneYocto)
    ));
}

#[test]
#[should_panic(expected = "council must not be this account")]
fn init_rejects_a_council_that_is_the_extension_itself() {
    ctx(ADMIN, 0);
    let _ = HosExtension::new(
        acc(ADMIN),
        acc(REGISTRY),
        acc(RECOVERY),
        acc(DEST),
        acc("hos-extension.testnet"),
    );
}

#[test]
fn a_pause_does_not_block_a_renewal_reaching_the_wallet() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    c.pause().unwrap();
    ctx(REGISTRY, 0);
    assert!(
        c.push_lease(acc(WALLET), U64(1), OperatingState::Active)
            .is_ok(),
        "the registry does not pause renewal, so pausing the push would take a \
         holder's rent and leave the wallet lease behind"
    );
}

mod treasury_rotation {
    use super::*;

    const NEW_TREASURY: &str = "treasury2.testnet";

    #[test]
    fn the_skim_destination_follows_a_committed_rotation() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        c.approve_treasury_rotation(acc(NEW_TREASURY)).unwrap();
        assert_eq!(c.get_treasury(), acc(DEST));
        assert_eq!(c.pending_treasury(), Some((acc(NEW_TREASURY), U64(0))));
        ctx_at(NEW_TREASURY, 1, UPGRADE_DELAY_NS);
        c.commit_treasury_rotation().unwrap();
        assert_eq!(
            c.get_treasury(),
            acc(NEW_TREASURY),
            "the registry can rotate its treasury, so skim must be able to follow or revenue \
             strands on the old account forever"
        );
        assert!(c.pending_treasury().is_none());
    }

    #[test]
    fn a_rotation_cannot_commit_inside_its_delay() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        c.approve_treasury_rotation(acc(NEW_TREASURY)).unwrap();
        ctx_at(NEW_TREASURY, 1, UPGRADE_DELAY_NS - 1);
        assert!(matches!(
            c.commit_treasury_rotation(),
            Err(ContractError::TreasuryRotationTooYoung)
        ));
        assert_eq!(c.get_treasury(), acc(DEST));
    }

    #[test]
    fn an_account_that_never_claimed_it_does_not_become_the_treasury() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        c.approve_treasury_rotation(acc(NEW_TREASURY)).unwrap();
        ctx_at(COUNCIL, 1, UPGRADE_DELAY_NS);
        assert!(matches!(
            c.commit_treasury_rotation(),
            Err(ContractError::OnlyPendingTreasury)
        ));
        assert_eq!(c.get_treasury(), acc(DEST));
    }

    #[test]
    fn an_approval_can_be_withdrawn_before_it_commits() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        c.approve_treasury_rotation(acc(NEW_TREASURY)).unwrap();
        ctx_at(COUNCIL, 1, 1);
        c.cancel_treasury_rotation().unwrap();
        assert!(c.pending_treasury().is_none());
        ctx_at(NEW_TREASURY, 1, UPGRADE_DELAY_NS);
        assert!(matches!(
            c.commit_treasury_rotation(),
            Err(ContractError::NoTreasuryRotationPending)
        ));
        assert_eq!(c.get_treasury(), acc(DEST));
    }

    #[test]
    fn only_the_council_can_move_the_treasury() {
        let mut c = deploy();
        ctx_at(ADMIN, 1, 0);
        assert!(matches!(
            c.approve_treasury_rotation(acc(NEW_TREASURY)),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn the_rotation_refuses_a_no_op_or_the_contract_itself() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        assert!(matches!(
            c.approve_treasury_rotation(acc(DEST)),
            Err(ContractError::TreasuryUnchanged)
        ));
        assert!(matches!(
            c.approve_treasury_rotation(near_sdk::env::current_account_id()),
            Err(ContractError::TreasuryIsSelf)
        ));
    }
}

mod council_rotation {
    use super::*;

    const NEW_COUNCIL: &str = "council2.testnet";

    #[test]
    fn a_rotation_installs_the_new_council_once_the_delay_has_run() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        c.approve_council_rotation(acc(NEW_COUNCIL)).unwrap();
        assert_eq!(c.pending_council(), Some((acc(NEW_COUNCIL), U64(0))));
        ctx_at(NEW_COUNCIL, 1, UPGRADE_DELAY_NS);
        c.commit_council_rotation().unwrap();
        assert_eq!(c.get_council(), acc(NEW_COUNCIL));
        assert!(c.pending_council().is_none());
    }

    #[test]
    fn the_outgoing_council_cannot_seat_an_account_that_never_signed() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        c.approve_council_rotation(acc(NEW_COUNCIL)).unwrap();
        ctx_at(COUNCIL, 1, UPGRADE_DELAY_NS);
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
        ctx_at(COUNCIL, 1, 0);
        c.approve_council_rotation(acc(NEW_COUNCIL)).unwrap();
        ctx_at(NEW_COUNCIL, 1, UPGRADE_DELAY_NS - 1);
        assert!(matches!(
            c.commit_council_rotation(),
            Err(ContractError::CouncilRotationTooYoung)
        ));
        assert_eq!(c.get_council(), acc(COUNCIL));
    }

    #[test]
    fn an_approval_can_be_withdrawn_before_it_commits() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        c.approve_council_rotation(acc(NEW_COUNCIL)).unwrap();
        ctx_at(COUNCIL, 1, 1);
        c.cancel_council_rotation().unwrap();
        ctx_at(COUNCIL, 1, UPGRADE_DELAY_NS);
        assert!(matches!(
            c.commit_council_rotation(),
            Err(ContractError::NoCouncilRotationPending)
        ));
        assert_eq!(c.get_council(), acc(COUNCIL));
    }

    #[test]
    fn only_the_council_can_move_the_council() {
        let mut c = deploy();
        ctx_at(ADMIN, 1, 0);
        assert!(matches!(
            c.approve_council_rotation(acc(NEW_COUNCIL)),
            Err(ContractError::OnlyCouncil)
        ));
    }

    #[test]
    fn the_rotation_refuses_a_council_that_would_end_the_gate() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        assert!(matches!(
            c.approve_council_rotation(acc(COUNCIL)),
            Err(ContractError::CouncilUnchanged)
        ));
        ctx_at(COUNCIL, 1, 0);
        assert!(matches!(
            c.approve_council_rotation(acc("hos-extension.testnet")),
            Err(ContractError::CouncilIsSelf)
        ));
    }

    #[test]
    fn moving_the_council_takes_a_full_access_signature() {
        let mut c = deploy();
        ctx_at(COUNCIL, 0, 0);
        assert!(matches!(
            c.approve_council_rotation(acc(NEW_COUNCIL)),
            Err(ContractError::RequiresOneYocto)
        ));
    }

    #[test]
    fn the_new_council_gates_what_the_old_one_used_to() {
        let mut c = deploy();
        ctx_at(COUNCIL, 1, 0);
        c.approve_council_rotation(acc(NEW_COUNCIL)).unwrap();
        ctx_at(NEW_COUNCIL, 1, UPGRADE_DELAY_NS);
        c.commit_council_rotation().unwrap();
        ctx_at(COUNCIL, 1, UPGRADE_DELAY_NS);
        assert!(matches!(
            c.add_admin(acc("late.testnet")),
            Err(ContractError::OnlyCouncil)
        ));
        ctx_at(NEW_COUNCIL, 1, UPGRADE_DELAY_NS);
        assert!(c.add_admin(acc("late.testnet")).is_ok());
    }
}

mod legacy_state {
    use super::*;

    fn as_v1(c: HosExtension) -> crate::legacy::HosExtensionV1 {
        crate::legacy::HosExtensionV1 {
            state_version: 1,
            admins: c.admins,
            registry: c.registry,
            recovery: c.recovery,
            paused: c.paused,
            version: c.version,
            treasury: c.treasury,
            approved_code_hash: c.approved_code_hash,
            approved_at: c.approved_at,
            council: c.council,
            paused_until_ns: c.paused_until_ns,
            recovery_reset_pending: c.recovery_reset_pending,
            upgrade_proven: c.upgrade_proven,
        }
    }

    #[test]
    fn a_state_left_at_version_one_migrates_through_its_own_reader() {
        let c = deploy();
        let registry = c.registry.clone();
        let council = c.council.clone();
        let old = as_v1(c);
        ctx("hos-extension.testnet", 0);
        env::state_write(&old);
        drop(old);

        let migrated = HosExtension::migrate();
        assert_eq!(migrated.state_version, STATE_VERSION);
        assert_eq!(migrated.registry, registry);
        assert_eq!(migrated.council, council);
        assert!(migrated.pending_council.is_none());
        assert!(
            migrated.admins.contains(&acc(ADMIN)),
            "the shape changed, the data did not"
        );
    }
}

#[test]
fn the_state_layout_is_pinned_to_the_version_that_declares_it() {
    let c = deploy();
    assert_eq!(
        (STATE_VERSION, near_sdk::borsh::to_vec(&c).unwrap().len()),
        (4, 154),
        "the state shape moved. Bump STATE_VERSION, add a reader in legacy.rs for \
         the shape that is deployed today, and update this fixture. A publish that \
         skips that leaves migrate unable to read what is on the account."
    );
}

#[test]
fn a_sweep_that_does_nothing_returns_the_deposit_to_whoever_paid_it() {
    let mut c = deploy();
    ctx(REGISTRY, sweep_deposit());
    let payer = acc("payer.testnet");
    let _ = c.after_balance_for_sweep(
        acc(WALLET),
        acc(TOKEN),
        acc(DEST),
        payer.clone(),
        Ok(U128(0)),
    );
    let refunds: Vec<(AccountId, u128)> = near_sdk::test_utils::get_created_receipts()
        .into_iter()
        .flat_map(|r| {
            let to = r.receiver_id.clone();
            r.actions.into_iter().filter_map(move |a| match a {
                near_sdk::mock::MockAction::Transfer { deposit, .. } => {
                    Some((to.clone(), deposit.as_yoctonear()))
                }
                _ => None,
            })
        })
        .collect();
    assert_eq!(
        refunds,
        vec![(payer, sweep_deposit())],
        "the registry is only the relay, so resting the storage deposit there takes \
         it from the account that actually paid"
    );
}
