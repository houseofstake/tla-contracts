use super::*;
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::testing_env;
use std::str::FromStr;

const IMPL: &str = "w.hos.testnet";
const COUNCIL: &str = "council.testnet";
const PATCH: &str = "patch.testnet";

fn acc(s: &str) -> AccountId {
    AccountId::from_str(s).unwrap()
}

fn ctx(predecessor: &str, deposit: u128) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(acc(IMPL))
        .predecessor_account_id(acc(predecessor))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .build());
}

const AFTER_DELAY: u64 = DEFAULT_APPROVAL_DELAY_NS + 1;

fn ctx_at(predecessor: &str, deposit: u128, ts: u64) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(acc(IMPL))
        .predecessor_account_id(acc(predecessor))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .block_timestamp(ts)
        .build());
}

fn deploy() -> ImplDeployer {
    ctx(COUNCIL, 0);
    ImplDeployer::new(acc(COUNCIL), None)
}

#[test]
#[should_panic(expected = "council must not be this account")]
fn init_rejects_a_council_that_is_the_deployer_itself() {
    ctx(COUNCIL, 0);
    let _ = ImplDeployer::new(acc(IMPL), None);
}

fn key_delete_callback(result: near_sdk::PromiseResult) {
    testing_env!(
        VMContextBuilder::new()
            .current_account_id(acc(IMPL))
            .predecessor_account_id(acc(IMPL))
            .build(),
        near_sdk::test_vm_config(),
        near_sdk::RuntimeFeesConfig::test(),
        Default::default(),
        vec![result],
    );
}

#[test]
fn a_key_that_survived_deletion_is_not_reported_as_deleted() {
    let mut c = deploy();
    key_delete_callback(near_sdk::PromiseResult::Failed);
    assert!(!c.gd_on_key_deleted("ed25519:key".to_string(), acc(COUNCIL)));
    let logs = near_sdk::test_utils::get_logs();
    assert!(logs.iter().any(|l| l.contains("key_delete_failed")));
    assert!(
        !logs.iter().any(|l| l.contains(r#""event":"key_deleted""#)),
        "the launch gate reads this log, so a key that survived must never read as removed"
    );
}

fn code() -> Base64VecU8 {
    Base64VecU8::from(vec![7u8; 64])
}

fn code_hash() -> Base58CryptoHash {
    Base58CryptoHash::from(near_sdk::env::sha256_array(&code().0))
}

fn cost() -> u128 {
    64 * GLOBAL_CODE_COST_PER_BYTE
}

#[test]
fn council_approves_and_anyone_deploys() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.gd_approve(code_hash());
    assert_eq!(c.approved_hash(), Some(code_hash()));
    ctx_at("anyone.testnet", cost(), AFTER_DELAY);
    let _ = c.gd_deploy(code());
    ctx(IMPL, 0);
    assert!(c.gd_on_deployed(
        code_hash(),
        64,
        acc("anyone.testnet"),
        NearToken::from_yoctonear(cost()),
        NearToken::from_yoctonear(cost()),
        Ok(()),
    ));
    assert_eq!(c.current_hash(), Some(code_hash()));
    assert_eq!(c.approved_hash(), None);
}

#[test]
#[should_panic(expected = "only council")]
fn a_second_approver_key_no_longer_exists() {
    let mut c = deploy();
    ctx(PATCH, 1);
    c.gd_approve(code_hash());
}

#[test]
#[should_panic(expected = "only council")]
fn outsider_cannot_approve() {
    let mut c = deploy();
    ctx("attacker.testnet", 1);
    c.gd_approve(code_hash());
}

#[test]
#[should_panic(expected = "no approved code hash")]
fn deploy_without_approval_rejected() {
    let mut c = deploy();
    ctx("anyone.testnet", cost());
    let _ = c.gd_deploy(code());
}

#[test]
#[should_panic(expected = "code does not match the approved hash")]
fn deploy_wrong_code_rejected() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.gd_approve(code_hash());
    ctx("anyone.testnet", cost());
    let _ = c.gd_deploy(Base64VecU8::from(vec![8u8; 64]));
}

#[test]
#[should_panic(expected = "attached deposit below global storage cost")]
fn deploy_underfunded_rejected() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.gd_approve(code_hash());
    ctx_at("anyone.testnet", cost() - 1, AFTER_DELAY);
    let _ = c.gd_deploy(code());
}

#[test]
#[should_panic(expected = "approved code must wait out the delay before publishing")]
fn deploy_before_the_delay_rejected() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.gd_approve(code_hash());
    ctx_at("anyone.testnet", cost(), DEFAULT_APPROVAL_DELAY_NS - 1);
    let _ = c.gd_deploy(code());
}

#[test]
#[should_panic(expected = "another deploy is in flight")]
fn concurrent_deploy_rejected() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.gd_approve(code_hash());
    ctx_at("anyone.testnet", cost(), AFTER_DELAY);
    let _ = c.gd_deploy(code());
    ctx_at("anyone.testnet", cost(), AFTER_DELAY);
    let _ = c.gd_deploy(code());
}

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
    ctx(IMPL, 0);
    env::state_write(&c);
    ImplDeployer::migrate();
}

#[test]
fn a_deploy_whose_callback_never_lands_stops_blocking_after_the_ttl() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.gd_approve(code_hash());
    ctx_at("anyone.testnet", cost(), AFTER_DELAY);
    let _ = c.gd_deploy(code());
    ctx_at("anyone.testnet", cost(), AFTER_DELAY + DEPLOY_LOCK_TTL_NS);
    let _ = c.gd_deploy(code());
    assert_eq!(
        c.deploy_locked_until().0,
        AFTER_DELAY + 2 * DEPLOY_LOCK_TTL_NS
    );
}

#[test]
fn failed_deploy_clears_flight_and_keeps_approval_consumable() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.gd_approve(code_hash());
    ctx_at("anyone.testnet", cost(), AFTER_DELAY);
    let _ = c.gd_deploy(code());
    ctx(IMPL, 0);
    assert!(!c.gd_on_deployed(
        code_hash(),
        64,
        acc("anyone.testnet"),
        NearToken::from_yoctonear(cost()),
        NearToken::from_yoctonear(cost()),
        Err(PromiseError::Failed),
    ));
    assert_eq!(c.current_hash(), None);
    assert_eq!(c.approved_hash(), Some(code_hash()));
    ctx_at("anyone.testnet", cost(), AFTER_DELAY);
    let _ = c.gd_deploy(code());
}

#[test]
#[should_panic(expected = "only council")]
fn self_upgrade_rejects_a_non_council_caller() {
    let mut c = deploy();
    ctx(PATCH, 1);
    let _ = c.upgrade_self(code());
}

#[test]
#[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
fn approval_rejects_a_restricted_access_key() {
    let mut c = deploy();
    ctx(COUNCIL, 0);
    c.gd_approve(code_hash());
}

#[test]
#[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
fn self_upgrade_rejects_a_restricted_access_key() {
    let mut c = deploy();
    ctx(COUNCIL, 0);
    let _ = c.upgrade_self(code());
}

#[test]
fn deploy_cost_is_linear() {
    let c = deploy();
    assert_eq!(
        c.deploy_cost(200_000).as_yoctonear(),
        200_000u128 * GLOBAL_CODE_COST_PER_BYTE
    );
}

#[test]
#[should_panic(expected = "no approved upgrade hash")]
fn a_self_upgrade_without_an_approval_is_refused() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    let _ = c.upgrade_self(code());
}

#[test]
#[should_panic(expected = "must wait out the delay")]
fn a_self_upgrade_inside_the_window_is_refused() {
    let mut c = deploy();
    ctx_at(COUNCIL, 1, 0);
    c.approve_self_upgrade(code_hash());
    ctx_at(COUNCIL, 1, DEFAULT_APPROVAL_DELAY_NS - 1);
    let _ = c.upgrade_self(code());
}

#[test]
#[should_panic(expected = "does not match the approved upgrade hash")]
fn a_self_upgrade_of_different_code_is_refused() {
    let mut c = deploy();
    ctx_at(COUNCIL, 1, 0);
    c.approve_self_upgrade(code_hash());
    ctx_at(COUNCIL, 1, AFTER_DELAY);
    let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(vec![9u8; 64]));
}

#[test]
fn an_approved_self_upgrade_installs_once_the_window_passes() {
    let mut c = deploy();
    ctx_at(COUNCIL, 1, 0);
    c.approve_self_upgrade(code_hash());
    assert_eq!(c.approved_upgrade_hash(), Some(code_hash()));
    ctx_at(COUNCIL, 1, AFTER_DELAY);
    let _ = c.upgrade_self(code());
    assert!(
        c.approved_upgrade_hash().is_none(),
        "the approval is spent by the upgrade it authorised"
    );
}

#[test]
fn the_council_removes_the_key_once_the_upgrade_path_is_proven() {
    let mut c = deploy();
    c.upgrade_proven = true;
    ctx(COUNCIL, 1);
    let key: near_sdk::PublicKey = "ed25519:6E8sCci9badyRkXb3JoRpBj5p8C6Tw41ELDZoiihKEtp"
        .parse()
        .unwrap();
    let _ = c.gd_delete_key(key.clone());
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
        vec![String::from(&key)],
        "this account decides the code under every tenant wallet, so its key removal is the one \
         the launch gate cannot take on trust"
    );
}

#[test]
#[should_panic(expected = "only council")]
fn only_the_council_may_remove_a_key() {
    let mut c = deploy();
    ctx("anyone.testnet", 1);
    let _ = c.gd_delete_key(
        "ed25519:6E8sCci9badyRkXb3JoRpBj5p8C6Tw41ELDZoiihKEtp"
            .parse()
            .unwrap(),
    );
}

#[test]
#[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
fn removing_a_key_needs_a_full_access_signature() {
    let mut c = deploy();
    ctx(COUNCIL, 0);
    let _ = c.gd_delete_key(
        "ed25519:6E8sCci9badyRkXb3JoRpBj5p8C6Tw41ELDZoiihKEtp"
            .parse()
            .unwrap(),
    );
}

#[test]
fn migrate_drops_a_stuck_deploy_flag_and_a_pending_approval() {
    ctx(IMPL, 0);
    env::state_write(&ImplDeployer {
        state_version: crate::STATE_VERSION,
        council: acc(COUNCIL),
        current_hash: Some([3u8; 32]),
        approved_hash: Some([4u8; 32]),
        approved_at: Some(7),
        approval_delay_ns: 11,
        deploy_locked_until: 99,
        approved_upgrade_hash: None,
        approved_upgrade_at: None,
        upgrade_proven: false,
        pending_council: None,
        pending_council_at: None,
    });
    let migrated = ImplDeployer::migrate();
    assert_eq!(migrated.council, acc(COUNCIL));
    assert_eq!(migrated.current_hash, Some([3u8; 32]));
    assert_eq!(migrated.approval_delay_ns, 11);
    assert_eq!(
        migrated.deploy_locked_until, 0,
        "a publish stuck in flight must not carry across an upgrade"
    );
    assert!(
        migrated.approved_hash.is_none(),
        "an approval must not survive the code it was granted against"
    );
}

#[test]
fn state_already_current_survives_a_same_shape_redeploy() {
    ctx(COUNCIL, 0);
    let current = deploy();
    let delay = current.approval_delay_ns;
    env::state_write(&current);
    assert_eq!(ImplDeployer::migrate().approval_delay_ns, delay);
}

#[test]
#[should_panic(expected = "no state to migrate")]
fn an_account_with_no_state_refuses_rather_than_writing_a_default() {
    ctx(COUNCIL, 0);
    let _ = ImplDeployer::migrate();
}

const NEW_COUNCIL: &str = "council2.testnet";

#[test]
fn a_rotation_installs_the_new_council_once_the_delay_has_run() {
    let mut c = deploy();
    ctx_at(COUNCIL, 1, 0);
    c.approve_council_rotation(acc(NEW_COUNCIL));
    assert_eq!(
        c.pending_council(),
        Some((acc(NEW_COUNCIL), near_sdk::json_types::U64(0)))
    );
    ctx_at(NEW_COUNCIL, 1, AFTER_DELAY);
    c.commit_council_rotation();
    assert_eq!(c.config().council, acc(NEW_COUNCIL));
    assert!(c.pending_council().is_none());
}

#[test]
#[should_panic(expected = "council rotation must wait out the delay")]
fn a_rotation_cannot_commit_inside_its_delay() {
    let mut c = deploy();
    ctx_at(COUNCIL, 1, 0);
    c.approve_council_rotation(acc(NEW_COUNCIL));
    ctx_at(NEW_COUNCIL, 1, DEFAULT_APPROVAL_DELAY_NS - 1);
    c.commit_council_rotation();
}

#[test]
#[should_panic(expected = "only the incoming council")]
fn the_outgoing_council_cannot_seat_an_account_that_never_signed() {
    let mut c = deploy();
    ctx_at(COUNCIL, 1, 0);
    c.approve_council_rotation(acc(NEW_COUNCIL));
    ctx_at(COUNCIL, 1, AFTER_DELAY);
    c.commit_council_rotation();
}

#[test]
#[should_panic(expected = "no council rotation has been approved")]
fn an_approval_can_be_withdrawn_before_it_commits() {
    let mut c = deploy();
    ctx_at(COUNCIL, 1, 0);
    c.approve_council_rotation(acc(NEW_COUNCIL));
    ctx_at(COUNCIL, 1, 1);
    c.cancel_council_rotation();
    assert!(c.pending_council().is_none());
    ctx_at(COUNCIL, 1, AFTER_DELAY);
    c.commit_council_rotation();
}

#[test]
#[should_panic(expected = "only council")]
fn only_the_council_can_move_the_council() {
    let mut c = deploy();
    ctx_at(PATCH, 1, 0);
    c.approve_council_rotation(acc(NEW_COUNCIL));
}

#[test]
#[should_panic(expected = "council must not be this account")]
fn the_rotation_refuses_a_council_that_would_end_the_gate() {
    let mut c = deploy();
    ctx_at(COUNCIL, 1, 0);
    c.approve_council_rotation(acc(IMPL));
}

#[test]
#[should_panic(expected = "exactly 1 yoctoNEAR")]
fn moving_the_council_takes_a_full_access_signature() {
    let mut c = deploy();
    ctx_at(COUNCIL, 0, 0);
    c.approve_council_rotation(acc(NEW_COUNCIL));
}

#[test]
fn the_new_council_gates_what_the_old_one_used_to() {
    let mut c = deploy();
    ctx_at(COUNCIL, 1, 0);
    c.approve_council_rotation(acc(NEW_COUNCIL));
    ctx_at(NEW_COUNCIL, 1, AFTER_DELAY);
    c.commit_council_rotation();
    c.gd_approve(Base58CryptoHash::from([5u8; 32]));
    assert_eq!(
        c.approved_hash(),
        Some(Base58CryptoHash::from([5u8; 32])),
        "the incoming council must hold the gate the outgoing one handed over"
    );
}

fn as_v1(c: ImplDeployer) -> crate::legacy::ImplDeployerV1 {
    crate::legacy::ImplDeployerV1 {
        state_version: 1,
        council: c.council,
        current_hash: c.current_hash,
        approved_hash: c.approved_hash,
        approved_at: c.approved_at,
        approval_delay_ns: c.approval_delay_ns,
        deploy_locked_until: c.deploy_locked_until,
        approved_upgrade_hash: c.approved_upgrade_hash,
        approved_upgrade_at: c.approved_upgrade_at,
        upgrade_proven: c.upgrade_proven,
    }
}

#[test]
fn a_state_left_at_version_one_migrates_through_its_own_reader() {
    let mut c = deploy();
    c.current_hash = Some([8u8; 32]);
    let delay = c.approval_delay_ns;
    let old = as_v1(c);
    ctx(IMPL, 0);
    env::state_write(&old);
    drop(old);

    let migrated = ImplDeployer::migrate();
    assert_eq!(migrated.state_version, STATE_VERSION);
    assert_eq!(migrated.council, acc(COUNCIL));
    assert_eq!(migrated.approval_delay_ns, delay);
    assert_eq!(
        migrated.current_hash,
        Some([8u8; 32]),
        "the published implementation must survive the shape change"
    );
    assert!(migrated.pending_council.is_none());
}

fn transfers() -> Vec<(AccountId, u128)> {
    near_sdk::test_utils::get_created_receipts()
        .into_iter()
        .flat_map(|receipt| {
            let receiver = receipt.receiver_id.clone();
            receipt
                .actions
                .into_iter()
                .filter_map(move |action| match action {
                    near_sdk::mock::MockAction::Transfer { deposit, .. } => {
                        Some((receiver.clone(), deposit.as_yoctonear()))
                    }
                    _ => None,
                })
        })
        .collect()
}

fn ctx_with_balance(predecessor: &str, deposit: u128, balance: u128) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(acc(IMPL))
        .predecessor_account_id(acc(predecessor))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .account_balance(NearToken::from_yoctonear(balance))
        .build());
}

#[test]
fn a_publish_refunds_against_what_it_actually_spent() {
    let mut c = deploy();
    let payer = acc("anyone.testnet");
    let before = 10 * cost();
    let real_spend = cost() / 4;
    ctx_with_balance(IMPL, 0, before - real_spend);
    assert!(c.gd_on_deployed(
        code_hash(),
        64,
        payer.clone(),
        NearToken::from_yoctonear(cost()),
        NearToken::from_yoctonear(before),
        Ok(()),
    ));
    assert_eq!(
        transfers(),
        vec![(payer, cost() - real_spend)],
        "the constant is a floor on what the caller attaches, so a publish that \
         cost less than it must not refund the difference out of this account"
    );
}

#[test]
fn a_publish_that_cost_more_than_was_attached_refunds_nothing() {
    let mut c = deploy();
    let payer = acc("anyone.testnet");
    let before = 10 * cost();
    ctx_with_balance(IMPL, 0, before - 2 * cost());
    assert!(c.gd_on_deployed(
        code_hash(),
        64,
        payer,
        NearToken::from_yoctonear(cost()),
        NearToken::from_yoctonear(before),
        Ok(()),
    ));
    assert!(
        transfers().is_empty(),
        "an underestimate must stop the refund, not run it negative"
    );
}

#[test]
fn the_state_layout_is_pinned_to_the_version_that_declares_it() {
    let c = deploy();
    assert_eq!(
        (
            crate::STATE_VERSION,
            near_sdk::borsh::to_vec(&c).unwrap().len()
        ),
        (2, 45),
        "the state shape moved. Bump STATE_VERSION, add a reader in legacy.rs for \
         the shape that is deployed today, and update this fixture. A publish that \
         skips that leaves migrate unable to read what is on the account."
    );
}
