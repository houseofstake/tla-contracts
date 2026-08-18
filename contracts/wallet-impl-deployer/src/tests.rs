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

const DEPLOYED_STATE_B64: &str = "FwAAAGNvdW5jaWwuaG9zZGVtby50ZXN0bmV0ART/FN3rvfqH9Axe9UfExYK0Vp58RqnJhaYw6b2EEkG4AAAAAAAAAAAAAAAAAAAAAAAAAAA=";

#[test]
fn the_legacy_struct_still_matches_the_deployed_state() {
    use near_sdk::base64::Engine;
    use near_sdk::borsh::BorshDeserialize;
    let raw = near_sdk::base64::engine::general_purpose::STANDARD
        .decode(DEPLOYED_STATE_B64)
        .expect("fixture is valid base64");
    let old = LegacyImplDeployer::try_from_slice(&raw)
        .expect("deployed state no longer decodes as LegacyImplDeployer");
    assert_eq!(old.council.as_str(), "council.hosdemo.testnet");
    assert!(old.current_hash.is_some());
    assert_eq!(old.deploy_locked_until, 0);
    assert!(old.approved_upgrade_hash.is_none());
    assert!(
        ImplDeployer::try_from_slice(&raw).is_ok(),
        "legacy and current are the same shape today, so both arms parse and migrate is a \
         no-op remap. The moment a field is added this must flip to only legacy parsing, or \
         the upgrade panics on a keyless account"
    );
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
fn the_legacy_arm_runs_and_drops_a_stuck_deploy_flag() {
    ctx(COUNCIL, 0);
    env::state_write(&LegacyImplDeployer {
        council: acc(COUNCIL),
        current_hash: Some([3u8; 32]),
        approved_hash: Some([4u8; 32]),
        approved_at: Some(7),
        approval_delay_ns: 11,
        deploy_locked_until: 99,
        approved_upgrade_hash: None,
        approved_upgrade_at: None,
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
