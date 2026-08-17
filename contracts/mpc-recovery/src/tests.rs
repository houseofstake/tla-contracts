use super::*;
use ed25519_dalek::{Signer, SigningKey};
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::{testing_env, CurveType};
use rand::rngs::OsRng;
use std::str::FromStr;

const OWNER: &str = "hos.testnet";
const CONTRACT: &str = "mpc-recovery.testnet";
const SIGNER: &str = "v1.signer-prod.testnet";
const VICTIM: &str = "victim.testnet";

fn keypair() -> (SigningKey, PublicKey) {
    let sk = SigningKey::generate(&mut OsRng);
    let pk =
        PublicKey::from_parts(CurveType::ED25519, sk.verifying_key().to_bytes().to_vec()).unwrap();
    (sk, pk)
}

fn ctx(predecessor: &str, ts: u64, height: u64) {
    let acct = AccountId::from_str(predecessor).unwrap();
    testing_env!(VMContextBuilder::new()
        .current_account_id(AccountId::from_str(CONTRACT).unwrap())
        .predecessor_account_id(acct)
        .block_timestamp(ts)
        .block_height(height)
        .build());
}

fn mpc_public_key() -> PublicKey {
    PublicKey::from_str("ed25519:DZdWKDt29SBdPqeyfykg8TFF5Zkb5Qzdd6FJiJMvftZG").unwrap()
}

const TRANSFER_AUTHORITY: &str = "hos-extension.testnet";

fn deploy(watcher_keys: &[PublicKey], threshold: u32) -> MpcRecovery {
    ctx(OWNER, 0, 0);
    MpcRecovery::new(
        AccountId::from_str(OWNER).unwrap(),
        AccountId::from_str(SIGNER).unwrap(),
        AccountId::from_str(TRANSFER_AUTHORITY).unwrap(),
        watcher_keys.to_vec(),
        threshold,
    )
}

fn account_id() -> AccountId {
    AccountId::from_str(VICTIM).unwrap()
}

fn install(c: &mut MpcRecovery, attestation_key: PublicKey) {
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), attestation_key, 60);
}

fn attest(sk: &SigningKey, new_owner: &PublicKey, round: u64) -> Base64VecU8 {
    let msg = proof::request_message(
        &AccountId::from_str(CONTRACT).unwrap(),
        &account_id(),
        new_owner,
        round,
    );
    Base64VecU8::from(sk.sign(&msg).to_bytes().to_vec())
}

fn watcher_sigs(
    sks: &[&SigningKey],
    pks: &[PublicKey],
    new_owner: &PublicKey,
    round: u64,
    approve: bool,
) -> Vec<WatcherSignature> {
    let msg = proof::verdict_message(
        &AccountId::from_str(CONTRACT).unwrap(),
        &account_id(),
        new_owner,
        round,
        approve,
    );
    sks.iter()
        .zip(pks)
        .map(|(sk, pk)| WatcherSignature {
            public_key: pk.clone(),
            signature: Base64VecU8::from(sk.sign(&msg).to_bytes().to_vec()),
        })
        .collect()
}

const TLA: &str = "mytla.testnet";
const REGISTRY: &str = "registry.testnet";

fn name_sigs(
    sks: &[&SigningKey],
    pks: &[PublicKey],
    new_owner: &AccountId,
    expected_owner: &AccountId,
    deadline_ns: u64,
) -> Vec<WatcherSignature> {
    let msg = proof::name_recovery_message(
        &AccountId::from_str(CONTRACT).unwrap(),
        &AccountId::from_str(TLA).unwrap(),
        "alice",
        new_owner,
        expected_owner,
        deadline_ns,
    );
    sks.iter()
        .zip(pks)
        .map(|(sk, pk)| WatcherSignature {
            public_key: pk.clone(),
            signature: Base64VecU8::from(sk.sign(&msg).to_bytes().to_vec()),
        })
        .collect()
}

fn deploy_with_registry(watcher_keys: &[PublicKey], threshold: u32) -> MpcRecovery {
    let mut c = deploy(watcher_keys, threshold);
    ctx(OWNER, 0, 0);
    testing_env!(VMContextBuilder::new()
        .current_account_id(AccountId::from_str(CONTRACT).unwrap())
        .predecessor_account_id(AccountId::from_str(OWNER).unwrap())
        .attached_deposit(near_sdk::NearToken::from_yoctonear(1))
        .build());
    c.set_registry(AccountId::from_str(REGISTRY).unwrap());
    c
}

fn name_ctx(ts: u64) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(AccountId::from_str(CONTRACT).unwrap())
        .predecessor_account_id(AccountId::from_str("anyone.testnet").unwrap())
        .attached_deposit(near_sdk::NearToken::from_yoctonear(1))
        .block_timestamp(ts)
        .build());
}

#[test]
fn a_name_recovery_needs_a_watcher_quorum() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let mut c = deploy_with_registry(&[wk1.clone(), wk2.clone()], 2);
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let expected = AccountId::from_str(VICTIM).unwrap();
    let sigs = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &expected, 1_000);
    name_ctx(500);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        expected,
        U64(1_000),
        sigs,
    );
}

#[test]
#[should_panic(expected = "watcher quorum not met")]
fn one_watcher_cannot_recover_a_name_alone() {
    let (w1, wk1) = keypair();
    let (_w2, wk2) = keypair();
    let mut c = deploy_with_registry(&[wk1.clone(), wk2], 2);
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let expected = AccountId::from_str(VICTIM).unwrap();
    let sigs = name_sigs(&[&w1], &[wk1], &new_owner, &expected, 1_000);
    name_ctx(500);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        expected,
        U64(1_000),
        sigs,
    );
}

#[test]
#[should_panic(expected = "quorum for this recovery has expired")]
fn a_stale_quorum_cannot_be_replayed_later() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let mut c = deploy_with_registry(&[wk1.clone(), wk2.clone()], 2);
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let expected = AccountId::from_str(VICTIM).unwrap();
    let sigs = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &expected, 1_000);
    name_ctx(1_001);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        expected,
        U64(1_000),
        sigs,
    );
}

#[test]
#[should_panic(expected = "watcher quorum not met")]
fn a_quorum_for_one_holder_does_not_move_a_name_held_by_another() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let mut c = deploy_with_registry(&[wk1.clone(), wk2.clone()], 2);
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let signed_for = AccountId::from_str(VICTIM).unwrap();
    let sigs = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &signed_for, 1_000);
    name_ctx(500);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        AccountId::from_str("someone-else.testnet").unwrap(),
        U64(1_000),
        sigs,
    );
}

#[test]
#[should_panic(expected = "no registry configured")]
fn name_recovery_is_refused_until_the_registry_is_wired() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let mut c = deploy(&[wk1.clone(), wk2.clone()], 2);
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let expected = AccountId::from_str(VICTIM).unwrap();
    let sigs = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &expected, 1_000);
    name_ctx(500);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        expected,
        U64(1_000),
        sigs,
    );
}

#[test]
fn happy_path_to_approved() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone(), wk2.clone()], 2);
    install(&mut c, mother_pk);

    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );

    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(
        account_id(),
        Verdict::Approve,
        watcher_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, 0, true),
    );

    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
    assert_eq!(c.round_of(account_id()), Some(1));
}

#[test]
#[should_panic(expected = "invalid attestation")]
fn request_rejects_forged_attestation() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let (wrong, _) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&wrong, &new_owner, 0),
    );
}

#[test]
#[should_panic(expected = "stale or invalid round")]
fn request_rejects_wrong_round() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(7),
        attest(&mother, &new_owner, 7),
    );
}

#[test]
#[should_panic(expected = "recovery already in progress")]
fn request_rejects_when_active() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 2, 2);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(1),
        attest(&mother, &new_owner, 1),
    );
}

#[test]
#[should_panic(expected = "timelock not elapsed")]
fn verdict_rejected_before_timelock() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 2, 2);
    let _ = c.submit_verdict(
        account_id(),
        Verdict::Approve,
        watcher_sigs(&[&w1], &[wk1], &new_owner, 0, true),
    );
}

#[test]
#[should_panic(expected = "watcher quorum not met")]
fn verdict_rejected_without_quorum() {
    let (w1, wk1) = keypair();
    let (_, wk2) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone(), wk2], 2);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(
        account_id(),
        Verdict::Approve,
        watcher_sigs(&[&w1], &[wk1], &new_owner, 0, true),
    );
}

#[test]
fn active_verdict_cancels_back_to_idle() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(
        account_id(),
        Verdict::Cancel,
        watcher_sigs(&[&w1], &[wk1], &new_owner, 0, false),
    );
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
#[should_panic(expected = "recovery not approved")]
fn finalize_rejected_without_approval() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let zeros = Base58CryptoHash::from([0u8; 32]);
    let _ = c.finalize_recovery(account_id(), U64(1), zeros);
}

fn approve_recovery(
    c: &mut MpcRecovery,
    mother: &SigningKey,
    w1: &SigningKey,
    wk1: PublicKey,
) -> PublicKey {
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(
        account_id(),
        Verdict::Approve,
        watcher_sigs(&[w1], &[wk1], &new_owner, 0, true),
    );
    new_owner
}

fn block_hash() -> Base58CryptoHash {
    Base58CryptoHash::from([0u8; 32])
}

#[test]
fn a_quorum_verdict_approves_without_touching_a_wallet() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let out = c.submit_verdict(
        account_id(),
        Verdict::Approve,
        watcher_sigs(&[&w1], &[wk1], &new_owner, 0, true),
    );
    assert!(matches!(out, PromiseOrValue::Value(())));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
}

#[test]
fn finalize_moves_an_approved_recovery_to_resolving() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Resolving { .. }
    ));
}

#[test]
#[should_panic(expected = "recovery not approved")]
fn second_finalize_rejected_while_resolving() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    let _ = c.finalize_recovery(account_id(), U64(2), block_hash());
}

#[test]
#[should_panic(expected = "only installer or owner")]
fn finalize_rejects_an_unauthorized_caller() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_recovery(&mut c, &mother, &w1, wk1);
    ctx("anyone.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
}

#[test]
fn abort_from_requested_returns_to_idle() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx(OWNER, 0, 2);
    let out = c.abort_recovery(account_id());
    assert!(matches!(out, PromiseOrValue::Value(())));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
fn abort_from_approved_returns_to_idle() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 0, 6);
    let out = c.abort_recovery(account_id());
    assert!(matches!(out, PromiseOrValue::Value(())));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
#[should_panic(expected = "only installer or owner")]
fn abort_rejects_an_unauthorized_caller() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_recovery(&mut c, &mother, &w1, wk1);
    ctx("attacker.testnet", 0, 6);
    let _ = c.abort_recovery(account_id());
}

#[test]
#[should_panic(expected = "no abortable recovery")]
fn abort_rejects_when_idle() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    ctx(OWNER, 0, 2);
    let _ = c.abort_recovery(account_id());
}

#[test]
#[should_panic(expected = "no abortable recovery")]
fn abort_rejects_while_resolving() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    ctx(OWNER, 0, 7);
    let _ = c.abort_recovery(account_id());
}

#[test]
fn transfer_resets_idle_policy() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    ctx(TRANSFER_AUTHORITY, 0, 1);
    c.on_wallet_transferred(account_id());
    assert!(c.accounts.get(&account_id()).is_none());
}

#[test]
fn transfer_resets_requested_policy() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx(TRANSFER_AUTHORITY, 0, 2);
    c.on_wallet_transferred(account_id());
    assert!(c.accounts.get(&account_id()).is_none());
}

#[test]
fn transfer_disarms_an_approved_recovery() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_recovery(&mut c, &mother, &w1, wk1);
    ctx(TRANSFER_AUTHORITY, 0, 6);
    c.on_wallet_transferred(account_id());
    assert!(
        c.accounts.get(&account_id()).is_none(),
        "an approved recovery must not survive a change of owner"
    );
}

#[test]
fn transfer_preserves_the_round_floor_so_rounds_never_replay() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk.clone());
    approve_recovery(&mut c, &mother, &w1, wk1);
    let round = c.accounts.get(&account_id()).unwrap().round;
    ctx(TRANSFER_AUTHORITY, 0, 6);
    c.on_wallet_transferred(account_id());
    install(&mut c, mother_pk);
    assert_eq!(
        c.accounts.get(&account_id()).unwrap().round,
        round,
        "a reinstalled policy resumes at the preserved round"
    );
}

#[test]
#[should_panic(expected = "only transfer authority")]
fn transfer_rejects_non_authority() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    ctx("attacker.testnet", 0, 1);
    c.on_wallet_transferred(account_id());
}

fn approve_native_recovery(
    c: &mut MpcRecovery,
    mother: &SigningKey,
    w1: &SigningKey,
    wk1: PublicKey,
) -> PublicKey {
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(
        account_id(),
        Verdict::Approve,
        watcher_sigs(&[w1], &[wk1], &new_owner, 0, true),
    );
    new_owner
}

#[test]
fn native_finalize_signs_and_resolves() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
    ctx(OWNER, 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Resolving { .. }
    ));
}

#[test]
fn native_signature_keeps_round_until_owner_claims() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    let out = c.on_signed(
        account_id(),
        0,
        "ab".to_string(),
        Ok(json!({"signature": "stub"})),
    );
    assert!(out.is_some());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
    ctx(OWNER, 62 * NS_PER_SEC, 7);
    c.claim_native_finalized(account_id(), U64(0));
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
fn native_finalize_retryable_after_signature() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    let _ = c.on_signed(
        account_id(),
        0,
        "ab".to_string(),
        Ok(json!({"signature": "stub"})),
    );
    ctx(OWNER, 62 * NS_PER_SEC, 7);
    let _ = c.finalize_recovery(account_id(), U64(2), block_hash());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Resolving { .. }
    ));
}

#[test]
fn native_on_signed_failure_restores_approved() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    let out = c.on_signed(account_id(), 0, "ab".to_string(), Err(PromiseError::Failed));
    assert!(out.is_none());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Approved { .. }
    ));
}

#[test]
#[should_panic(expected = "only installer or owner")]
fn native_finalize_rejects_unauthorized() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    ctx("attacker.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
}

#[test]
#[should_panic(expected = "only installer or owner")]
fn native_claim_rejects_unauthorized() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    let _ = c.on_signed(
        account_id(),
        0,
        "ab".to_string(),
        Ok(json!({"signature": "stub"})),
    );
    ctx("attacker.testnet", 62 * NS_PER_SEC, 7);
    c.claim_native_finalized(account_id(), U64(0));
}

#[test]
#[should_panic(expected = "recovery not approved")]
fn native_claim_rejects_wrong_round() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    let _ = c.on_signed(
        account_id(),
        0,
        "ab".to_string(),
        Ok(json!({"signature": "stub"})),
    );
    ctx(OWNER, 62 * NS_PER_SEC, 7);
    c.claim_native_finalized(account_id(), U64(5));
}

#[test]
fn reinstall_preserves_round_when_idle() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx(OWNER, 0, 2);
    let _ = c.abort_recovery(account_id());
    assert_eq!(c.round_of(account_id()), Some(1));
    let (_, mother_pk2) = keypair();
    install(&mut c, mother_pk2);
    assert_eq!(
        c.round_of(account_id()),
        Some(1),
        "reinstall must preserve the monotonic round, not reset it to 0"
    );
}

#[test]
#[should_panic(expected = "recovery already in progress")]
fn reinstall_rejected_while_in_flight() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    let (_, mother_pk2) = keypair();
    ctx(OWNER, 0, 2);
    c.install_policy(account_id(), mpc_public_key(), mother_pk2, 60);
}

#[test]
#[should_panic(expected = "timelock above maximum")]
fn install_rejects_a_timelock_that_would_brick_recovery() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, u32::MAX);
}

#[test]
#[should_panic(expected = "timelock below minimum")]
fn install_rejects_zero_timelock() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, 0);
}

#[test]
#[should_panic(expected = "duplicate watcher key")]
fn new_rejects_duplicate_watchers() {
    let (_, wk1) = keypair();
    let _ = deploy(&[wk1.clone(), wk1], 1);
}

#[test]
#[should_panic(expected = "watcher quorum not met")]
fn verdict_over_wrong_new_owner_rejected() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    let (_, other) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx("anyone.testnet", 61 * NS_PER_SEC, 5);
    let _ = c.submit_verdict(
        account_id(),
        Verdict::Approve,
        watcher_sigs(&[&w1], &[wk1], &other, 0, true),
    );
}

#[test]
fn round_preserved_across_transfer_and_reinstall() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    let (_, new_owner) = keypair();
    ctx("anyone.testnet", 1, 1);
    c.request_recovery(
        account_id(),
        new_owner.clone(),
        U64(0),
        attest(&mother, &new_owner, 0),
    );
    ctx(TRANSFER_AUTHORITY, 0, 2);
    c.on_wallet_transferred(account_id());
    assert!(c.accounts.get(&account_id()).is_none());
    let (_, mother_pk2) = keypair();
    install(&mut c, mother_pk2);
    assert_eq!(
        c.round_of(account_id()),
        Some(1),
        "round must not reset to 0 across transfer+reinstall (verdict-replay defense)"
    );
}

#[test]
fn expected_native_path_is_account_scoped() {
    let (_, wk1) = keypair();
    let c = deploy(&[wk1], 1);
    let a = AccountId::from_str("alice.tla.testnet").unwrap();
    let b = AccountId::from_str("bob.tla.testnet").unwrap();
    assert_ne!(c.expected_native_path(a.clone()), c.expected_native_path(b));
    assert_eq!(c.expected_native_path(a), "hos-recovery/alice.tla.testnet");
}

const INSTALLER: &str = "installer.testnet";

#[test]
fn installer_defaults_to_the_owner_on_deploy() {
    let (_, wk1) = keypair();
    let c = deploy(&[wk1], 1);
    assert_eq!(c.installer(), c.owner());
}

#[test]
fn set_installer_delegates_and_owner_keeps_authority() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx(OWNER, 0, 0);
    c.set_installer(AccountId::from_str(INSTALLER).unwrap());
    assert_eq!(c.installer(), AccountId::from_str(INSTALLER).unwrap());
    assert_eq!(c.owner(), AccountId::from_str(OWNER).unwrap());
}

#[test]
#[should_panic(expected = "only owner")]
fn set_installer_rejects_a_non_owner() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx("attacker.testnet", 0, 0);
    c.set_installer(AccountId::from_str("attacker.testnet").unwrap());
}

#[test]
fn a_delegated_installer_can_install_a_policy() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx(OWNER, 0, 0);
    c.set_installer(AccountId::from_str(INSTALLER).unwrap());
    ctx(INSTALLER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, 259_200);
    assert_eq!(c.timelock_of(account_id()), Some(259_200));
}

#[test]
fn the_owner_can_still_install_after_delegating() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx(OWNER, 0, 0);
    c.set_installer(AccountId::from_str(INSTALLER).unwrap());
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, 259_200);
    assert_eq!(c.timelock_of(account_id()), Some(259_200));
}

#[test]
#[should_panic(expected = "only installer or owner")]
fn install_rejects_an_unauthorized_caller() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx("attacker.testnet", 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, 259_200);
}

#[test]
fn a_delegated_installer_can_finalize_an_approved_recovery() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 61 * NS_PER_SEC, 6);
    c.set_installer(AccountId::from_str(INSTALLER).unwrap());
    ctx(INSTALLER, 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Resolving { .. }
    ));
}

#[test]
fn a_delegated_installer_can_abort_a_recovery() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(std::slice::from_ref(&wk1), 1);
    install(&mut c, mother_pk);
    approve_recovery(&mut c, &mother, &w1, wk1);
    ctx(OWNER, 0, 6);
    c.set_installer(AccountId::from_str(INSTALLER).unwrap());
    ctx(INSTALLER, 0, 6);
    let _ = c.abort_recovery(account_id());
    assert!(matches!(
        c.accounts.get(&account_id()).unwrap().phase,
        Phase::Idle
    ));
}

#[test]
#[should_panic(expected = "only owner may replace an existing policy")]
fn a_delegated_installer_cannot_replace_an_existing_policy() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let (_, attacker_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    ctx(OWNER, 0, 0);
    c.set_installer(AccountId::from_str(INSTALLER).unwrap());
    ctx(INSTALLER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), attacker_pk, 259_200);
}

#[test]
fn the_owner_can_still_replace_an_existing_policy() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let (_, next_pk) = keypair();
    let mut c = deploy(&[wk1], 1);
    install(&mut c, mother_pk);
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), next_pk, 259_200);
    assert_eq!(c.timelock_of(account_id()), Some(259_200));
}

fn ctx_paying(predecessor: &str, ts: u64, deposit: u128) {
    let acct = AccountId::from_str(predecessor).unwrap();
    testing_env!(VMContextBuilder::new()
        .current_account_id(AccountId::from_str(CONTRACT).unwrap())
        .predecessor_account_id(acct)
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .block_timestamp(ts)
        .build());
}

fn code() -> Base64VecU8 {
    Base64VecU8(vec![7u8; 64])
}

fn code_hash() -> Base58CryptoHash {
    Base58CryptoHash::from(env::sha256_array(&code().0))
}

const AFTER_UPGRADE_DELAY: u64 = UPGRADE_DELAY_NS + 1;

#[test]
fn owner_approves_then_upgrades_after_the_delay() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx_paying(OWNER, 0, 1);
    c.approve_upgrade(code_hash());
    assert_eq!(c.approved_upgrade_hash(), Some(code_hash()));
    ctx_paying(OWNER, AFTER_UPGRADE_DELAY, 1);
    let _ = c.upgrade(code());
    assert_eq!(c.approved_upgrade_hash(), None);
    assert_eq!(c.approved_upgrade_at(), None);
}

#[test]
#[should_panic(expected = "approved code must wait out the delay")]
fn upgrade_before_the_delay_is_rejected() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx_paying(OWNER, 0, 1);
    c.approve_upgrade(code_hash());
    ctx_paying(OWNER, UPGRADE_DELAY_NS - 1, 1);
    let _ = c.upgrade(code());
}

#[test]
#[should_panic(expected = "code does not match the approved hash")]
fn upgrade_with_different_code_is_rejected() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx_paying(OWNER, 0, 1);
    c.approve_upgrade(code_hash());
    ctx_paying(OWNER, AFTER_UPGRADE_DELAY, 1);
    let _ = c.upgrade(Base64VecU8(vec![8u8; 64]));
}

#[test]
#[should_panic(expected = "no approved code hash")]
fn upgrade_without_an_approval_is_rejected() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx_paying(OWNER, AFTER_UPGRADE_DELAY, 1);
    let _ = c.upgrade(code());
}

#[test]
#[should_panic(expected = "only owner")]
fn a_non_owner_cannot_approve_an_upgrade() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx_paying("attacker.testnet", 0, 1);
    c.approve_upgrade(code_hash());
}

#[test]
#[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
fn a_restricted_key_cannot_approve_an_upgrade() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx_paying(OWNER, 0, 0);
    c.approve_upgrade(code_hash());
}

#[test]
fn the_owner_can_rotate_the_watcher_set() {
    let (_, wk1) = keypair();
    let (_, wk2) = keypair();
    let (_, wk3) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx_paying(OWNER, 0, 1);
    c.set_watchers(vec![wk2.clone(), wk3.clone()], 2);
    assert_eq!(c.watchers(), vec![wk2, wk3]);
    assert_eq!(c.threshold(), 2);
}

#[test]
#[should_panic(expected = "threshold must be at least 2")]
fn the_watcher_set_cannot_be_rotated_down_to_one() {
    let (_, wk1) = keypair();
    let (_, wk2) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx_paying(OWNER, 0, 1);
    c.set_watchers(vec![wk2], 1);
}

#[test]
#[should_panic(expected = "duplicate watcher key")]
fn the_watcher_set_rejects_a_duplicate_key() {
    let (_, wk1) = keypair();
    let (_, wk2) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx_paying(OWNER, 0, 1);
    c.set_watchers(vec![wk2.clone(), wk2], 2);
}

#[test]
#[should_panic(expected = "only owner")]
fn a_non_owner_cannot_rotate_the_watcher_set() {
    let (_, wk1) = keypair();
    let (_, wk2) = keypair();
    let (_, wk3) = keypair();
    let mut c = deploy(&[wk1], 1);
    ctx_paying("attacker.testnet", 0, 1);
    c.set_watchers(vec![wk2, wk3], 2);
}

mod migration {
    use crate::LegacyMpcRecovery;
    use near_sdk::base64::Engine;
    use near_sdk::borsh::BorshDeserialize;

    const DEPLOYED_STATE_B64: &str = "FwAAAGNvdW5jaWwuaG9zZGVtby50ZXN0bmV0DgAAAGhvc3RsYS50ZXN0bmV0FgAAAHYxLnNpZ25lci1wcm9kLnRlc3RuZXQTAAAAZXh0Lmhvc2RlbW8udGVzdG5ldAEAAAAhAAAAAIS/j9urBasuVw52LoAoxzfkejeW2buaWME8f8jHrqeAAQAAAAEAAABhAQAAAHI=";

    #[test]
    fn the_legacy_struct_still_matches_the_deployed_state() {
        let raw = near_sdk::base64::engine::general_purpose::STANDARD
            .decode(DEPLOYED_STATE_B64)
            .expect("fixture is valid base64");
        let old = LegacyMpcRecovery::try_from_slice(&raw)
            .expect("deployed state no longer decodes as LegacyMpcRecovery");
        assert_eq!(old.owner.as_str(), "council.hosdemo.testnet");
        assert_eq!(old.installer.as_str(), "hostla.testnet");
        assert_eq!(old.signer.as_str(), "v1.signer-prod.testnet");
        assert_eq!(old.transfer_authority.as_str(), "ext.hosdemo.testnet");
        assert_eq!(old.watchers.len(), 1);
        assert_eq!(old.threshold, 1);
    }
}

#[test]
fn the_legacy_arm_runs_and_keeps_the_watcher_set() {
    let (_, wk1) = keypair();
    let (_, wk2) = keypair();
    ctx_paying(OWNER, 0, 0);
    env::state_write(&crate::LegacyMpcRecovery {
        owner: AccountId::from_str(OWNER).unwrap(),
        installer: AccountId::from_str(OWNER).unwrap(),
        signer: AccountId::from_str(OWNER).unwrap(),
        transfer_authority: AccountId::from_str(OWNER).unwrap(),
        watchers: vec![wk1.clone(), wk2.clone()],
        threshold: 2,
        accounts: LookupMap::new(b"a"),
        round_floor: LookupMap::new(b"r"),
    });
    let migrated = MpcRecovery::migrate();
    assert_eq!(migrated.owner, AccountId::from_str(OWNER).unwrap());
    assert_eq!(
        migrated.threshold, 2,
        "dropping the threshold would let one watcher carry a recovery alone"
    );
    assert_eq!(migrated.watchers, vec![wk1, wk2]);
}

#[test]
fn state_already_current_survives_a_same_shape_redeploy() {
    let (_, wk1) = keypair();
    let c = deploy(&[wk1], 1);
    let owner = c.owner.clone();
    env::state_write(&c);
    assert_eq!(MpcRecovery::migrate().owner, owner);
}

fn seal_ctx(predecessor: &str, deposit: u128) {
    testing_env!(VMContextBuilder::new()
        .current_account_id(AccountId::from_str(CONTRACT).unwrap())
        .predecessor_account_id(AccountId::from_str(predecessor).unwrap())
        .attached_deposit(near_sdk::NearToken::from_yoctonear(deposit))
        .build());
}

#[test]
fn the_owner_can_seal_the_recovery_contract() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1], 1);
    seal_ctx(OWNER, 1);
    let _ = c.seal(mpc_public_key());
}

#[test]
#[should_panic(expected = "only owner")]
fn an_owner_that_cannot_call_cannot_remove_the_key() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1], 1);
    seal_ctx("attacker.testnet", 1);
    let _ = c.seal(mpc_public_key());
}

#[test]
#[should_panic]
fn sealing_the_recovery_contract_needs_a_full_access_signature() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1], 1);
    seal_ctx(OWNER, 0);
    let _ = c.seal(mpc_public_key());
}

#[test]
#[should_panic(expected = "owner must not be this account")]
fn init_rejects_an_owner_that_is_the_recovery_contract_itself() {
    let (_, wk1) = keypair();
    ctx(OWNER, 0, 0);
    let _ = MpcRecovery::new(
        AccountId::from_str(CONTRACT).unwrap(),
        AccountId::from_str(SIGNER).unwrap(),
        AccountId::from_str(TRANSFER_AUTHORITY).unwrap(),
        vec![wk1],
        1,
    );
}
