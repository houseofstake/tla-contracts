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
        min_label_len: 3,
        wallet_timeout_secs: 3600,
    })
}

fn min() -> u128 {
    NearToken::from_millinear(100).as_yoctonear()
}

fn lease() -> u64 {
    TS + YEAR_NS
}

#[test]
fn registry_can_mint() {
    let mut c = deploy();
    ctx(REGISTRY, min());
    let _ = c.create_sub_account("alice".to_string(), acc(OWNER_ACC), acc(PAYOUT), lease());
}

#[test]
#[should_panic(expected = "only registry")]
fn outsider_cannot_mint() {
    let mut c = deploy();
    ctx("attacker.testnet", min());
    let _ = c.create_sub_account("alice".to_string(), acc(OWNER_ACC), acc(PAYOUT), lease());
}

#[test]
#[should_panic(expected = "name shorter than the minimum label length")]
fn short_name_rejected() {
    let mut c = deploy();
    ctx(REGISTRY, min());
    let _ = c.create_sub_account("ab".to_string(), acc(OWNER_ACC), acc(PAYOUT), lease());
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
fn council_sets_min_label_len() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.set_min_label_len(4);
    assert_eq!(c.min_label_len(), 4);
}

#[test]
#[should_panic(expected = "only council")]
fn registry_cannot_set_min_label_len() {
    let mut c = deploy();
    ctx(REGISTRY, 1);
    c.set_min_label_len(4);
}

#[test]
#[should_panic(expected = "min label length out of bounds")]
fn zero_min_label_len_rejected() {
    let mut c = deploy();
    ctx(COUNCIL, 1);
    c.set_min_label_len(0);
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
        min_label_len: 3,
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

const DEPLOYED_STATE_B64: &str = "GAAAAHJlZ2lzdHJ5Lmhvc2RlbW8udGVzdG5ldBcAAABjb3VuY2lsLmhvc2RlbW8udGVzdG5ldBQAAABpbXBsLmhvc2RlbW8udGVzdG5ldBMAAABleHQuaG9zZGVtby50ZXN0bmV0EwAAAHJlYy5ob3NkZW1vLnRlc3RuZXQHAAAAdGVzdG5ldAAAYBZpwIN4ewEAAAAAAAADEA4AAAAA";

#[test]
fn the_config_epoch_moves_on_anything_that_invalidates_a_cached_proof() {
    let mut c = deploy();
    assert_eq!(c.config_epoch(), 0);
    ctx(COUNCIL, 1);
    c.set_min_label_len(4);
    assert_eq!(c.config_epoch(), 1);
    ctx(COUNCIL, 1);
    c.set_min_balance(NearToken::from_millinear(200));
    assert_eq!(
        c.config_epoch(),
        2,
        "a consumer caching the namespace proof has no other signal that this account's \
         minting behaviour changed under it"
    );
}

#[test]
fn an_upgrade_moves_the_config_epoch() {
    ctx(COUNCIL, 0);
    env::state_write(&LegacyRegistrar {
        registry: acc(REGISTRY),
        council: acc(COUNCIL),
        wallet_impl: acc(IMPL),
        hos_extension: acc(EXTENSION),
        recovery: acc(RECOVERY),
        chain_id: "testnet".to_string(),
        min_balance: NearToken::from_millinear(100),
        min_label_len: 3,
        wallet_timeout_secs: 3600,
        approved_code_hash: None,
        approved_at: None,
    });
    assert_eq!(Registrar::migrate().config_epoch(), 1);
}

#[test]
fn the_deployed_state_decodes_as_legacy_so_migrate_can_read_it() {
    use near_sdk::base64::Engine;
    use near_sdk::borsh::BorshDeserialize;
    let raw = near_sdk::base64::engine::general_purpose::STANDARD
        .decode(DEPLOYED_STATE_B64)
        .expect("fixture is valid base64");
    let old = LegacyRegistrar::try_from_slice(&raw)
        .expect("deployed state must decode as LegacyRegistrar or migrate cannot read it");
    assert_eq!(old.registry.as_str(), "registry.hosdemo.testnet");
    assert_eq!(old.council.as_str(), "council.hosdemo.testnet");
    assert_eq!(old.chain_id, "testnet");
    assert_eq!(old.min_label_len, 3);
    assert!(old.approved_code_hash.is_none());
}

fn a_key() -> PublicKey {
    PublicKey::from_str("ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847").unwrap()
}

#[test]
fn the_council_can_seal_the_root() {
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
        min_label_len: 3,
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
        min_label_len: 3,
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
