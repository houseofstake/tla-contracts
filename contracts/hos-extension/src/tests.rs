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
    HosExtension::new(acc(ADMIN), acc(REGISTRY), acc(RECOVERY))
}

fn sweep_deposit() -> u128 {
    MIN_SWEEP_ATTACHED.as_yoctonear()
}

#[test]
fn registry_sells_via_force_transfer() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(c
        .force_transfer(acc(WALLET), Some(acc(BUYER)), false)
        .is_ok());
}

#[test]
fn registry_parks_via_force_transfer() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(c.force_transfer(acc(WALLET), None, true).is_ok());
}

#[test]
fn park_with_new_owner_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.force_transfer(acc(WALLET), Some(acc(BUYER)), true),
        Err(ContractError::ParkTakesNoOwner)
    ));
}

#[test]
fn transfer_without_new_owner_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.force_transfer(acc(WALLET), None, false),
        Err(ContractError::TransferNeedsOwner)
    ));
}

#[test]
fn non_registry_cannot_force_transfer() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    assert!(matches!(
        c.force_transfer(acc(WALLET), Some(acc(BUYER)), false),
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
        c.force_transfer(acc(WALLET), Some(acc(BUYER)), false),
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
    assert!(c.skim(U128(1), acc(DEST)).is_ok());
}

#[test]
fn non_admin_cannot_skim() {
    let mut c = deploy();
    ctx(REGISTRY, 0);
    assert!(matches!(
        c.skim(U128(1), acc(DEST)),
        Err(ContractError::OnlyAdmin)
    ));
}

#[test]
fn admin_upgrade_returns_promise() {
    let mut c = deploy();
    ctx(ADMIN, 1);
    assert!(c.upgrade(vec![1, 2, 3]).is_ok());
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
    ctx(ADMIN, 1);
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
