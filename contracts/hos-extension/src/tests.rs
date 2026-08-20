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
fn skim_always_pays_the_treasury_fixed_at_deploy() {
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
        c.pending_recovery_resets(),
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
    assert!(c.pending_recovery_resets().is_empty());
}

#[test]
fn a_failed_reset_stays_pending() {
    let mut c = deploy();
    ctx(ADMIN, 0);
    c.after_recovery_reset(acc(WALLET), Err(near_sdk::PromiseError::Failed));
    assert_eq!(c.pending_recovery_resets(), vec![acc(WALLET)]);
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
