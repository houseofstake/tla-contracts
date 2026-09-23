use super::*;
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::testing_env;
use std::str::FromStr;

const REGISTRY: &str = "tla-registry.testnet";
const COUNCIL: &str = "council.testnet";
const IMPL: &str = "w.hos.testnet";
const EXTENSION: &str = "hos-extension.testnet";
const RECOVERY: &str = "mpc-recovery.testnet";
const PAYOUT: &str = "payout.testnet";
const OWNER_ACC: &str = "renter.testnet";
const TLA: &str = "tla.testnet";
const TS: u64 = 1_000_000_000_000;
const YEAR_NS: u64 = 31_536_000_000_000_000;

fn acc(s: &str) -> AccountId {
    AccountId::from_str(s).unwrap()
}

fn ctx(predecessor: &str, deposit: u128) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(acc(TLA))
        .predecessor_account_id(acc(predecessor))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .block_timestamp(TS)
        .build());
}

fn deploy() -> Registrar {
    ctx(COUNCIL, 0);
    Registrar::new(RegistrarConfig {
        registry: acc(REGISTRY),
        council: acc(COUNCIL),
        wallet_impl: acc(IMPL),
        hos_extension: acc(EXTENSION),
        recovery: acc(RECOVERY),
        chain_id: "testnet".to_string(),
        min_balance: NearToken::from_millinear(100),
        wallet_timeout_secs: 3600,
    })
}

fn seal_callback(result: near_sdk::PromiseResult) {
    testing_env!(
        VMContextBuilder::new()
            .current_account_id(acc(TLA))
            .predecessor_account_id(acc(TLA))
            .build(),
        near_sdk::test_vm_config(),
        near_sdk::RuntimeFeesConfig::test(),
        Default::default(),
        vec![result],
    );
}

#[test]
fn a_seal_that_did_not_remove_the_key_is_not_reported_as_sealed() {
    let mut c = deploy();
    seal_callback(near_sdk::PromiseResult::Failed);
    assert!(!c.after_seal("ed25519:key".to_string(), acc(COUNCIL)));
    let logs = near_sdk::test_utils::get_logs();
    assert!(logs.iter().any(|l| l.contains("seal_failed")));
    assert!(
        !logs.iter().any(|l| l.contains(r#""event":"sealed""#)),
        "the launch gate reads this log, so a key that survived must never read as sealed"
    );
}

fn min() -> u128 {
    NearToken::from_millinear(100).as_yoctonear()
}

fn lease() -> u64 {
    TS + YEAR_NS
}

fn minted_accounts() -> Vec<String> {
    near_sdk::test_utils::get_created_receipts()
        .into_iter()
        .filter(|receipt| {
            receipt
                .actions
                .iter()
                .any(|a| matches!(a, near_sdk::mock::MockAction::CreateAccount { .. }))
        })
        .map(|receipt| receipt.receiver_id.to_string())
        .collect()
}

#[test]
fn registry_can_mint() {
    let mut c = deploy();
    ctx(REGISTRY, min());
    let _ = c.create_sub_account("alice".to_string(), acc(OWNER_ACC), acc(PAYOUT), lease());
    assert_eq!(
        minted_accounts(),
        vec![format!("alice.{TLA}")],
        "only the account holding the TLA can create beneath it, so the name it builds is the \
         one invariant nothing else can correct"
    );
}

#[test]
#[should_panic(expected = "only registry")]
fn outsider_cannot_mint() {
    let mut c = deploy();
    ctx("attacker.testnet", min());
    let _ = c.create_sub_account("alice".to_string(), acc(OWNER_ACC), acc(PAYOUT), lease());
}

#[test]
fn a_one_character_name_is_accepted() {
    let mut c = deploy();
    ctx(REGISTRY, min());
    let _ = c.create_sub_account("a".to_string(), acc(OWNER_ACC), acc(PAYOUT), lease());
    assert_eq!(minted_accounts(), vec![format!("a.{TLA}")]);
}

#[test]
#[should_panic(expected = "invalid sub-account name")]
fn dotted_name_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, min());
    let _ = c.create_sub_account(
        "alice.bob".to_string(),
        acc(OWNER_ACC),
        acc(PAYOUT),
        lease(),
    );
}

#[test]
#[should_panic(expected = "invalid sub-account name")]
fn empty_name_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, min());
    let _ = c.create_sub_account(String::new(), acc(OWNER_ACC), acc(PAYOUT), lease());
}

#[test]
#[should_panic(expected = "deposit below minimum balance")]
fn underfunded_mint_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, min() - 1);
    let _ = c.create_sub_account("alice".to_string(), acc(OWNER_ACC), acc(PAYOUT), lease());
}

#[test]
#[should_panic(expected = "lease_until_ns must be in the future")]
fn past_lease_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, min());
    let _ = c.create_sub_account("alice".to_string(), acc(OWNER_ACC), acc(PAYOUT), TS);
}

#[test]
fn mint_success_reports_active() {
    let mut c = deploy();
    ctx(TLA, 0);
    let out = c.on_minted(
        acc("alice.tla.testnet"),
        acc(OWNER_ACC),
        NearToken::from_millinear(100),
        Ok(()),
    );
    assert!(matches!(out, PromiseOrValue::Value(MintOutcome::Active)));
}

#[test]
fn mint_failure_refunds_registry() {
    let mut c = deploy();
    ctx(TLA, 0);
    let out = c.on_minted(
        acc("alice.tla.testnet"),
        acc(OWNER_ACC),
        NearToken::from_millinear(100),
        Err(PromiseError::Failed),
    );
    assert!(matches!(out, PromiseOrValue::Promise(_)));
}

#[test]
fn the_upgrade_proven_flag_is_readable() {
    let mut c = deploy();
    assert!(!c.upgrade_proven());
    c.upgrade_proven = true;
    assert!(c.upgrade_proven());
}

#[test]
fn the_state_version_is_readable_from_chain() {
    let c = deploy();
    assert_eq!(
        c.state_version(),
        crate::STATE_VERSION,
        "an operator must be able to read which shape is on the account before an upgrade"
    );
}

#[test]
fn council_sets_min_balance_to_the_storage_floor() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.set_min_balance(ACCOUNT_STORAGE_FLOOR);
    assert_eq!(c.min_balance(), ACCOUNT_STORAGE_FLOOR);
}

#[test]
#[should_panic(expected = "only council")]
fn registry_cannot_set_min_balance() {
    let mut c = deploy();
    ctx(REGISTRY, 1);
    c.set_min_balance(NearToken::from_millinear(50));
}

#[test]
#[should_panic(expected = "min balance below the account storage floor")]
fn min_balance_below_storage_floor_rejected() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.set_min_balance(NearToken::from_millinear(1));
}

#[test]
#[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
fn a_restricted_key_cannot_set_min_balance() {
    let mut c = deploy();
    ctx(COUNCIL, 0);
    c.set_min_balance(ACCOUNT_STORAGE_FLOOR);
}

#[test]
#[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
fn a_restricted_key_cannot_approve_an_upgrade() {
    let mut c = deploy();
    ctx(COUNCIL, 0);
    c.approve_upgrade(hash_of(&[1]));
}

#[test]
#[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
fn a_restricted_key_cannot_upgrade_the_root() {
    let mut c = deploy();
    ctx(COUNCIL, 0);
    let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(vec![1]));
}

#[test]
#[should_panic(expected = "min balance below the account storage floor")]
fn init_below_storage_floor_rejected() {
    ctx(COUNCIL, 0);
    let _ = Registrar::new(RegistrarConfig {
        registry: acc(REGISTRY),
        council: acc(COUNCIL),
        wallet_impl: acc(IMPL),
        hos_extension: acc(EXTENSION),
        recovery: acc(RECOVERY),
        chain_id: "testnet".to_string(),
        min_balance: NearToken::from_millinear(1),
        wallet_timeout_secs: 3600,
    });
}

#[test]
#[should_panic(expected = "only council")]
fn outsider_cannot_upgrade() {
    let mut c = deploy();
    ctx("attacker.testnet", 1);
    let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(vec![1]));
}

fn ctx_at(predecessor: &str, ts: u64) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(acc(TLA))
        .predecessor_account_id(acc(predecessor))
        .attached_deposit(NearToken::from_yoctonear(1))
        .block_timestamp(ts)
        .build());
}

fn hash_of(code: &[u8]) -> near_sdk::json_types::Base58CryptoHash {
    near_sdk::env::sha256_array(code).into()
}

#[test]
#[should_panic(expected = "only council")]
fn outsider_cannot_approve_an_upgrade() {
    let mut c = deploy();
    ctx("attacker.testnet", 1);
    c.approve_upgrade(hash_of(&[1]));
}

#[test]
#[should_panic(expected = "no upgrade has been approved")]
fn the_root_cannot_be_upgraded_without_a_prior_approval() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(vec![1]));
}

#[test]
#[should_panic(expected = "code does not match the approved hash")]
fn approved_code_cannot_be_swapped_for_other_code() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.approve_upgrade(hash_of(&[1]));
    ctx_at(COUNCIL, TS + UPGRADE_DELAY_NS);
    let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(vec![2]));
}

#[test]
#[should_panic(expected = "approved upgrade is still inside its delay")]
fn the_root_cannot_be_upgraded_inside_the_delay() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.approve_upgrade(hash_of(&[1]));
    ctx_at(COUNCIL, TS + UPGRADE_DELAY_NS - 1);
    let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(vec![1]));
}

#[test]
fn the_config_epoch_moves_on_anything_that_invalidates_a_cached_proof() {
    let mut c = deploy();
    assert_eq!(c.config_epoch(), 0);
    ctx(COUNCIL, 1);
    c.set_min_balance(NearToken::from_millinear(200));
    assert_eq!(
        c.config_epoch(),
        1,
        "a consumer caching the namespace proof has no other signal that this account's \
         minting behaviour changed under it"
    );
}

#[test]
fn an_upgrade_moves_the_config_epoch() {
    let c = deploy();
    let before = c.config_epoch();
    ctx(TLA, 0);
    env::state_write(&c);
    assert_eq!(Registrar::migrate().config_epoch(), before + 1);
}

#[test]
#[should_panic(expected = "state version")]
fn migrate_refuses_a_state_version_it_does_not_understand() {
    let mut c = deploy();
    c.state_version = crate::STATE_VERSION + 1;
    ctx(TLA, 0);
    env::state_write(&c);
    Registrar::migrate();
}

fn a_key() -> PublicKey {
    PublicKey::from_str("ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847").unwrap()
}

#[test]
fn the_council_can_seal_the_root_once_the_upgrade_path_is_proven() {
    let mut c = deploy();
    c.upgrade_proven = true;
    ctx(COUNCIL, 1);
    let _ = c.seal(a_key());
    assert_eq!(
        deleted_keys(),
        vec![String::from(&a_key())],
        "the launch gate turns on this account ending with no key, so the seal has to schedule \
         the removal rather than only report one"
    );
}

fn deleted_keys() -> Vec<String> {
    near_sdk::test_utils::get_created_receipts()
        .into_iter()
        .flat_map(|receipt| receipt.actions)
        .filter_map(|action| match action {
            near_sdk::mock::MockAction::DeleteKey { public_key, .. } => {
                Some(public_key.to_string())
            }
            _ => None,
        })
        .collect()
}

#[test]
#[should_panic(expected = "upgrade path has not been exercised")]
fn sealing_the_root_is_refused_until_an_upgrade_has_run() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    let _ = c.seal(a_key());
}

#[test]
#[should_panic(expected = "only council")]
fn a_misconfigured_council_leaves_the_key_in_place() {
    let mut c = deploy();
    ctx("attacker.testnet", 1);
    let _ = c.seal(a_key());
}

#[test]
#[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
fn sealing_the_root_needs_a_full_access_signature() {
    let mut c = deploy();
    ctx(COUNCIL, 0);
    let _ = c.seal(a_key());
}

#[test]
#[should_panic(expected = "council must not be this account")]
fn init_rejects_a_council_that_is_the_root_itself() {
    ctx(COUNCIL, 0);
    let _ = Registrar::new(RegistrarConfig {
        registry: acc(REGISTRY),
        council: acc(TLA),
        wallet_impl: acc(IMPL),
        hos_extension: acc(EXTENSION),
        recovery: acc(RECOVERY),
        chain_id: "testnet".to_string(),
        min_balance: NearToken::from_millinear(100),
        wallet_timeout_secs: 3600,
    });
}

#[test]
#[should_panic(expected = "registry must not be this account")]
fn init_rejects_a_registry_that_is_the_root_itself() {
    ctx(COUNCIL, 0);
    let _ = Registrar::new(RegistrarConfig {
        registry: acc(TLA),
        council: acc(COUNCIL),
        wallet_impl: acc(IMPL),
        hos_extension: acc(EXTENSION),
        recovery: acc(RECOVERY),
        chain_id: "testnet".to_string(),
        min_balance: NearToken::from_millinear(100),
        wallet_timeout_secs: 3600,
    });
}

#[test]
fn an_approved_upgrade_lands_once_the_delay_has_passed() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.approve_upgrade(hash_of(&[1]));
    assert_eq!(c.approved_upgrade_hash(), Some(hash_of(&[1])));
    ctx_at(COUNCIL, TS + UPGRADE_DELAY_NS);
    let _ = c.upgrade_self(near_sdk::json_types::Base64VecU8(vec![1]));
    assert_eq!(
        c.approved_upgrade_hash(),
        None,
        "the approval is spent, so the same code cannot be redeployed unannounced"
    );
}

#[test]
fn config_roundtrips() {
    let c = deploy();
    let view = c.config();
    assert_eq!(view.council, acc(COUNCIL));
    assert_eq!(view.wallet_impl, acc(IMPL));
    assert_eq!(view.hos_extension, acc(EXTENSION));
    assert_eq!(view.recovery, acc(RECOVERY));
    assert_eq!(view.chain_id, "testnet");
    assert_eq!(view.wallet_timeout_secs, 3600);
}

const NEW_COUNCIL: &str = "council2.testnet";

#[test]
fn a_rotation_installs_the_new_council_once_the_delay_has_run() {
    let mut c = deploy();
    ctx_at(COUNCIL, TS);
    c.approve_council_rotation(acc(NEW_COUNCIL));
    assert_eq!(c.pending_council(), Some((acc(NEW_COUNCIL), U64(TS))));
    ctx_at(NEW_COUNCIL, TS + UPGRADE_DELAY_NS);
    c.commit_council_rotation();
    assert_eq!(c.config().council, acc(NEW_COUNCIL));
    assert!(c.pending_council().is_none());
}

#[test]
#[should_panic(expected = "council rotation must wait out the delay")]
fn a_rotation_cannot_commit_inside_its_delay() {
    let mut c = deploy();
    ctx_at(COUNCIL, TS);
    c.approve_council_rotation(acc(NEW_COUNCIL));
    ctx_at(NEW_COUNCIL, TS + UPGRADE_DELAY_NS - 1);
    c.commit_council_rotation();
}

#[test]
#[should_panic(expected = "only the incoming council")]
fn the_outgoing_council_cannot_seat_an_account_that_never_signed() {
    let mut c = deploy();
    ctx_at(COUNCIL, TS);
    c.approve_council_rotation(acc(NEW_COUNCIL));
    ctx_at(COUNCIL, TS + UPGRADE_DELAY_NS);
    c.commit_council_rotation();
}

#[test]
#[should_panic(expected = "no council rotation has been approved")]
fn an_approval_can_be_withdrawn_before_it_commits() {
    let mut c = deploy();
    ctx_at(COUNCIL, TS);
    c.approve_council_rotation(acc(NEW_COUNCIL));
    ctx_at(COUNCIL, TS + 1);
    c.cancel_council_rotation();
    assert!(c.pending_council().is_none());
    ctx_at(COUNCIL, TS + UPGRADE_DELAY_NS);
    c.commit_council_rotation();
}

#[test]
#[should_panic(expected = "only council")]
fn only_the_council_can_move_the_council() {
    let mut c = deploy();
    ctx_at(REGISTRY, TS);
    c.approve_council_rotation(acc(NEW_COUNCIL));
}

#[test]
#[should_panic(expected = "must differ from the current one")]
fn the_rotation_refuses_a_council_that_changes_nothing() {
    let mut c = deploy();
    ctx_at(COUNCIL, TS);
    c.approve_council_rotation(acc(COUNCIL));
}

#[test]
#[should_panic(expected = "council must not be this account")]
fn the_rotation_refuses_a_council_that_would_end_the_gate() {
    let mut c = deploy();
    ctx_at(COUNCIL, TS);
    c.approve_council_rotation(acc(TLA));
}

#[test]
#[should_panic(expected = "exactly 1 yoctoNEAR")]
fn moving_the_council_takes_a_full_access_signature() {
    let mut c = deploy();
    ctx(COUNCIL, 0);
    c.approve_council_rotation(acc(NEW_COUNCIL));
}

#[test]
fn the_new_council_gates_what_the_old_one_used_to() {
    let mut c = deploy();
    ctx_at(COUNCIL, TS);
    c.approve_council_rotation(acc(NEW_COUNCIL));
    ctx_at(NEW_COUNCIL, TS + UPGRADE_DELAY_NS);
    c.commit_council_rotation();
    c.set_min_balance(NearToken::from_millinear(200));
    assert_eq!(c.min_balance(), NearToken::from_millinear(200));
}

fn as_v1(c: Registrar) -> crate::legacy::RegistrarV1 {
    crate::legacy::RegistrarV1 {
        state_version: 1,
        registry: c.registry,
        council: c.council,
        wallet_impl: c.wallet_impl,
        hos_extension: c.hos_extension,
        recovery: c.recovery,
        chain_id: c.chain_id,
        min_balance: c.min_balance,
        min_label_len: 3,
        wallet_timeout_secs: c.wallet_timeout_secs,
        approved_code_hash: c.approved_code_hash,
        approved_at: c.approved_at,
        config_epoch: c.config_epoch,
        upgrade_proven: c.upgrade_proven,
    }
}

#[test]
fn a_state_left_at_version_one_migrates_through_its_own_reader() {
    let c = deploy();
    let registry = c.registry.clone();
    let council = c.council.clone();
    let old = as_v1(c);
    ctx(TLA, 0);
    env::state_write(&old);
    drop(old);

    let migrated = Registrar::migrate();
    assert_eq!(migrated.state_version, STATE_VERSION);
    assert_eq!(migrated.registry, registry);
    assert_eq!(
        migrated.council, council,
        "the minimum label length left the shape, the rest of the config did not"
    );
    assert!(migrated.pending_council.is_none());
}

#[test]
fn the_state_layout_is_pinned_to_the_version_that_declares_it() {
    let c = deploy();
    assert_eq!(
        (STATE_VERSION, near_sdk::borsh::to_vec(&c).unwrap().len()),
        (2, 151),
        "the state shape moved. Bump STATE_VERSION, add a reader in legacy.rs for \
         the shape that is deployed today, and update this fixture. A publish that \
         skips that leaves migrate unable to read what is on the account."
    );
}
