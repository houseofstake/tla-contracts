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
        .force_transfer(acc(WALLET), Some(acc(BUYER)), RotationCause::Sale)
        .is_ok());
}

#[test]
fn registry_parks_via_force_transfer() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(c
        .force_transfer(acc(WALLET), None, RotationCause::Reclaim)
        .is_ok());
}

#[test]
fn park_with_new_owner_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.force_transfer(acc(WALLET), Some(acc(BUYER)), RotationCause::Reclaim),
        Err(ContractError::ParkTakesNoOwner)
    ));
}

#[test]
fn transfer_without_new_owner_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.force_transfer(acc(WALLET), None, RotationCause::Sale),
        Err(ContractError::TransferNeedsOwner)
    ));
}

#[test]
fn non_registry_cannot_force_transfer() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    assert!(matches!(
        c.force_transfer(acc(WALLET), Some(acc(BUYER)), RotationCause::Sale),
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
        c.force_transfer(acc(WALLET), Some(acc(BUYER)), RotationCause::Sale),
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
    assert!(c.sweep_ft(acc(WALLET), acc(TOKEN)).is_ok());
}

#[test]
fn sweep_rejects_wrong_deposit() {
    let mut c = deploy();
    ctx(REGISTRY, sweep_deposit() - 1);
    assert!(matches!(
        c.sweep_ft(acc(WALLET), acc(TOKEN)),
        Err(ContractError::InsufficientDeposit)
    ));
}

#[test]
fn non_registry_cannot_sweep() {
    let mut c = deploy();
    ctx(ADMIN, sweep_deposit());
    assert!(matches!(
        c.sweep_ft(acc(WALLET), acc(TOKEN)),
        Err(ContractError::OnlyRegistry)
    ));
}

#[test]
fn admin_can_skim_within_available_balance() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    assert!(c.skim(U128(1)).is_ok());
}

#[test]
fn non_admin_cannot_skim() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(matches!(c.skim(U128(1)), Err(ContractError::OnlyAdmin)));
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
    assert!(c.upgrade(code).is_ok());
}

#[test]
fn upgrade_rejects_code_that_was_never_approved() {
    let mut c = deploy();
    ctx_at(ADMIN, 1, UPGRADE_DELAY_NS);
    assert!(matches!(
        c.upgrade(vec![1, 2, 3]),
        Err(ContractError::NoApprovedHash)
    ));
}

#[test]
fn upgrade_rejects_code_that_does_not_match_the_approved_hash() {
    let mut c = deploy();
    approve(&mut c, &[1, 2, 3], 0);
    ctx_at(ADMIN, 1, UPGRADE_DELAY_NS);
    assert!(matches!(
        c.upgrade(vec![9, 9, 9]),
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
        c.upgrade(code),
        Err(ContractError::ApprovalTooYoung)
    ));
}

#[test]
fn a_landed_upgrade_clears_the_approval_so_it_cannot_be_replayed() {
    let mut c = deploy();
    let code = vec![1, 2, 3];
    approve(&mut c, &code, 0);
    ctx_at(ADMIN, 1, UPGRADE_DELAY_NS);
    assert!(c.upgrade(code.clone()).is_ok());
    ctx_at(ADMIN, 1, UPGRADE_DELAY_NS * 2);
    assert!(matches!(
        c.upgrade(code),
        Err(ContractError::NoApprovedHash)
    ));
}

#[test]
fn skim_always_pays_the_treasury_fixed_at_deploy() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    assert!(c.skim(U128(1)).is_ok());
    assert_eq!(c.treasury, acc(DEST));
}

#[test]
fn upgrade_rejects_empty_code() {
    let mut c = deploy();
    ctx(ADMIN, 1);
    assert!(matches!(
        c.upgrade(Vec::new()),
        Err(ContractError::EmptyCode)
    ));
}

#[test]
fn non_admin_cannot_upgrade() {
    let mut c = deploy();
    ctx(REGISTRY, 1);
    assert!(matches!(c.upgrade(vec![1]), Err(ContractError::OnlyAdmin)));
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
        c.upgrade(vec![1]),
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
fn an_admin_cannot_approve_an_upgrade() {
    let mut c = deploy();
    ctx_at(ADMIN, 1, 0);
    assert!(matches!(
        c.approve_upgrade(Base58CryptoHash::from(env::sha256_array([1, 2, 3]))),
        Err(ContractError::OnlyCouncil)
    ));
}
