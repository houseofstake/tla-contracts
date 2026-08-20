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

fn ctx_yocto(predecessor: &str, ts: u64, height: u64) {
    let acct = AccountId::from_str(predecessor).unwrap();
    testing_env!(VMContextBuilder::new()
        .current_account_id(AccountId::from_str(CONTRACT).unwrap())
        .predecessor_account_id(acct)
        .attached_deposit(near_sdk::NearToken::from_yoctonear(1))
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

fn spare() -> &'static (SigningKey, PublicKey) {
    static SPARE: std::sync::OnceLock<(SigningKey, PublicKey)> = std::sync::OnceLock::new();
    SPARE.get_or_init(keypair)
}

fn spare_watcher() -> PublicKey {
    spare().1.clone()
}

fn account_id() -> AccountId {
    AccountId::from_str(VICTIM).unwrap()
}

fn install(c: &mut MpcRecovery, attestation_key: PublicKey) {
    ctx(VICTIM, 0, 0);
    c.arm_policy_install(attestation_key.clone(), 60);
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
        0,
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

const NAME_TIMELOCK_NS: u64 = 60 * 1_000_000_000;

fn leased_id() -> AccountId {
    AccountId::from_str(&format!("alice.{TLA}")).unwrap()
}

fn install_name_policy(c: &mut MpcRecovery, attestation_key: PublicKey) {
    ctx(leased_id().as_str(), 0, 0);
    c.arm_policy_install(attestation_key.clone(), 60);
    ctx(OWNER, 0, 0);
    c.install_policy(leased_id(), mpc_public_key(), attestation_key, 60);
}

fn request_name(c: &mut MpcRecovery, sk: &SigningKey, new_owner: &AccountId, round: u64) {
    let msg = proof::name_request_message(
        &AccountId::from_str(CONTRACT).unwrap(),
        &AccountId::from_str(TLA).unwrap(),
        "alice",
        new_owner,
        round,
    );
    ctx(OWNER, 0, 0);
    c.request_name_recovery(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner.clone(),
        Base64VecU8::from(sk.sign(&msg).to_bytes().to_vec()),
    );
}

fn armed_name_recovery(
    watcher_keys: &[PublicKey],
    threshold: u32,
    new_owner: &AccountId,
) -> (MpcRecovery, SigningKey) {
    let (attestor, attestor_pk) = keypair();
    let mut c = deploy_with_registry(watcher_keys, threshold);
    install_name_policy(&mut c, attestor_pk);
    request_name(&mut c, &attestor, new_owner, 0);
    (c, attestor)
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
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let (mut c, _) = armed_name_recovery(&[wk1.clone(), wk2.clone()], 2, &new_owner);
    let expected = AccountId::from_str(VICTIM).unwrap();
    let deadline = NAME_TIMELOCK_NS * 2;
    let sigs = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &expected, deadline);
    name_ctx(NAME_TIMELOCK_NS + 1);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        expected,
        U64(deadline),
        sigs,
    );
}

#[test]
#[should_panic(expected = "timelock has not elapsed")]
fn a_name_recovery_refuses_to_settle_inside_the_timelock() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let (mut c, _) = armed_name_recovery(&[wk1.clone(), wk2.clone()], 2, &new_owner);
    let expected = AccountId::from_str(VICTIM).unwrap();
    let deadline = NAME_TIMELOCK_NS * 2;
    let sigs = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &expected, deadline);
    name_ctx(NAME_TIMELOCK_NS - 1);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        expected,
        U64(deadline),
        sigs,
    );
}

#[test]
#[should_panic(expected = "no attested name recovery is pending")]
fn a_quorum_alone_cannot_move_a_name_without_an_attested_request() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let mut c = deploy_with_registry(&[wk1.clone(), wk2.clone()], 2);
    let (_, attestor_pk) = keypair();
    install_name_policy(&mut c, attestor_pk);
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let expected = AccountId::from_str(VICTIM).unwrap();
    let deadline = NAME_TIMELOCK_NS * 2;
    let sigs = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &expected, deadline);
    name_ctx(NAME_TIMELOCK_NS + 1);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        expected,
        U64(deadline),
        sigs,
    );
}

#[test]
#[should_panic(expected = "watcher quorum not met")]
fn a_quorum_signed_for_an_earlier_round_cannot_settle_a_later_one() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let (mut c, attestor) = armed_name_recovery(&[wk1.clone(), wk2.clone()], 2, &new_owner);
    let expected = AccountId::from_str(VICTIM).unwrap();
    let deadline = NAME_TIMELOCK_NS * 4;
    let stale = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &expected, deadline);
    ctx(OWNER, 0, 0);
    let _ = c.abort_recovery(leased_id());
    request_name(&mut c, &attestor, &new_owner, 1);
    name_ctx(NAME_TIMELOCK_NS + 1);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        expected,
        U64(deadline),
        stale,
    );
}

#[test]
fn a_failed_registry_settle_restores_the_attested_request() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let (mut c, _) = armed_name_recovery(&[wk1.clone(), wk2.clone()], 2, &new_owner);
    let expected = AccountId::from_str(VICTIM).unwrap();
    let deadline = NAME_TIMELOCK_NS * 4;
    let sigs = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &expected, deadline);
    name_ctx(NAME_TIMELOCK_NS + 1);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner.clone(),
        expected,
        U64(deadline),
        sigs,
    );
    ctx(CONTRACT, 0, 0);
    assert!(!c.on_name_recovered(leased_id(), 0, Err(PromiseError::Failed)));
    let entry = c.accounts.get(&leased_id()).unwrap();
    assert!(matches!(
        &entry.phase,
        Phase::NameRequested { new_owner: pending, round: 0, .. } if *pending == new_owner
    ));
}

#[test]
fn a_successful_registry_settle_clears_the_request() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let (mut c, _) = armed_name_recovery(&[wk1.clone(), wk2.clone()], 2, &new_owner);
    let expected = AccountId::from_str(VICTIM).unwrap();
    let deadline = NAME_TIMELOCK_NS * 4;
    let sigs = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &expected, deadline);
    name_ctx(NAME_TIMELOCK_NS + 1);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        expected,
        U64(deadline),
        sigs,
    );
    ctx(CONTRACT, 0, 0);
    assert!(c.on_name_recovered(leased_id(), 0, Ok(())));
    let entry = c.accounts.get(&leased_id()).unwrap();
    assert!(matches!(entry.phase, Phase::Idle));
}

#[test]
#[should_panic(expected = "no abortable recovery")]
fn a_name_recovery_in_flight_cannot_be_aborted() {
    let (w1, wk1) = keypair();
    let (w2, wk2) = keypair();
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let (mut c, _) = armed_name_recovery(&[wk1.clone(), wk2.clone()], 2, &new_owner);
    let expected = AccountId::from_str(VICTIM).unwrap();
    let deadline = NAME_TIMELOCK_NS * 4;
    let sigs = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &expected, deadline);
    name_ctx(NAME_TIMELOCK_NS + 1);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        expected,
        U64(deadline),
        sigs,
    );
    ctx(OWNER, 0, 0);
    let _ = c.abort_recovery(leased_id());
}

#[test]
#[should_panic(expected = "invalid attestation signature")]
fn a_name_request_refuses_an_attestation_from_the_wrong_key() {
    let (wk1, pk1) = keypair();
    let (_, pk2) = keypair();
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let mut c = deploy_with_registry(&[pk1, pk2], 2);
    let (_, attestor_pk) = keypair();
    install_name_policy(&mut c, attestor_pk);
    request_name(&mut c, &wk1, &new_owner, 0);
}

#[test]
#[should_panic(expected = "watcher quorum not met")]
fn one_watcher_cannot_recover_a_name_alone() {
    let (w1, wk1) = keypair();
    let (_w2, wk2) = keypair();
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let (mut c, _) = armed_name_recovery(&[wk1.clone(), wk2], 2, &new_owner);
    let expected = AccountId::from_str(VICTIM).unwrap();
    let deadline = NAME_TIMELOCK_NS * 2;
    let sigs = name_sigs(&[&w1], &[wk1], &new_owner, &expected, deadline);
    name_ctx(NAME_TIMELOCK_NS + 1);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        expected,
        U64(deadline),
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
    let new_owner = AccountId::from_str("bob.testnet").unwrap();
    let (mut c, _) = armed_name_recovery(&[wk1.clone(), wk2.clone()], 2, &new_owner);
    let signed_for = AccountId::from_str(VICTIM).unwrap();
    let deadline = NAME_TIMELOCK_NS * 2;
    let sigs = name_sigs(&[&w1, &w2], &[wk1, wk2], &new_owner, &signed_for, deadline);
    name_ctx(NAME_TIMELOCK_NS + 1);
    let _ = c.recover_name(
        AccountId::from_str(TLA).unwrap(),
        "alice".to_string(),
        new_owner,
        AccountId::from_str("someone-else.testnet").unwrap(),
        U64(deadline),
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
        watcher_sigs(
            &[&w1, &spare().0],
            &[wk1, spare_watcher()],
            &new_owner,
            0,
            true,
        ),
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
        watcher_sigs(
            &[&w1, &spare().0],
            &[wk1, spare_watcher()],
            &new_owner,
            0,
            true,
        ),
    );
}

#[test]
fn active_verdict_cancels_back_to_idle() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
        watcher_sigs(
            &[&w1, &spare().0],
            &[wk1, spare_watcher()],
            &new_owner,
            0,
            false,
        ),
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
        watcher_sigs(
            &[w1, &spare().0],
            &[wk1, spare_watcher()],
            &new_owner,
            0,
            true,
        ),
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
        watcher_sigs(
            &[&w1, &spare().0],
            &[wk1, spare_watcher()],
            &new_owner,
            0,
            true,
        ),
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
    install(&mut c, mother_pk);
    approve_recovery(&mut c, &mother, &w1, wk1);
    ctx("anyone.testnet", 61 * NS_PER_SEC, 6);
    let _ = c.finalize_recovery(account_id(), U64(1), block_hash());
}

#[test]
fn abort_from_requested_returns_to_idle() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
fn the_account_under_recovery_can_abort_its_own_case() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
    install(&mut c, mother_pk);
    approve_recovery(&mut c, &mother, &w1, wk1);
    ctx(VICTIM, 0, 6);
    let _ = c.abort_recovery(account_id());
    assert!(c.pending_target(account_id()).is_none());
}

#[test]
#[should_panic(expected = "only installer or owner")]
fn abort_rejects_an_unauthorized_caller() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    install(&mut c, mother_pk);
    ctx(OWNER, 0, 2);
    let _ = c.abort_recovery(account_id());
}

#[test]
#[should_panic(expected = "no abortable recovery")]
fn abort_rejects_while_resolving() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    install(&mut c, mother_pk);
    ctx(TRANSFER_AUTHORITY, 0, 1);
    c.on_wallet_transferred(account_id());
    assert!(c.accounts.get(&account_id()).is_none());
}

#[test]
fn transfer_resets_requested_policy() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
        watcher_sigs(
            &[w1, &spare().0],
            &[wk1, spare_watcher()],
            &new_owner,
            0,
            true,
        ),
    );
    new_owner
}

#[test]
fn native_finalize_signs_and_resolves() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, u32::MAX);
}

#[test]
#[should_panic(expected = "timelock below minimum")]
fn install_rejects_zero_timelock() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, 0);
}

#[test]
#[should_panic(expected = "threshold must be at least 2")]
fn init_rejects_a_threshold_one_watcher_could_meet_alone() {
    let (_, wk1) = keypair();
    let _ = deploy(&[wk1, spare_watcher()], 1);
}

#[test]
#[should_panic(expected = "duplicate watcher key")]
fn new_rejects_duplicate_watchers() {
    let (_, wk1) = keypair();
    let _ = deploy(&[wk1.clone(), wk1], 2);
}

#[test]
#[should_panic(expected = "watcher quorum not met")]
fn verdict_over_wrong_new_owner_rejected() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
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
        watcher_sigs(&[&w1, &spare().0], &[wk1, spare_watcher()], &other, 0, true),
    );
}

#[test]
fn round_preserved_across_transfer_and_reinstall() {
    let (_, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
    let c = deploy(&[wk1, spare_watcher()], 2);
    let a = AccountId::from_str("alice.tla.testnet").unwrap();
    let b = AccountId::from_str("bob.tla.testnet").unwrap();
    assert_ne!(c.expected_native_path(a.clone()), c.expected_native_path(b));
    assert_eq!(c.expected_native_path(a), "hos-recovery/alice.tla.testnet");
}

const INSTALLER: &str = "installer.testnet";

#[test]
fn installer_defaults_to_the_owner_on_deploy() {
    let (_, wk1) = keypair();
    let c = deploy(&[wk1, spare_watcher()], 2);
    assert_eq!(c.installer(), c.owner());
}

#[test]
fn set_installer_delegates_and_owner_keeps_authority() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_yocto(OWNER, 0, 0);
    c.set_installer(AccountId::from_str(INSTALLER).unwrap());
    assert_eq!(c.installer(), AccountId::from_str(INSTALLER).unwrap());
    assert_eq!(c.owner(), AccountId::from_str(OWNER).unwrap());
}

#[test]
#[should_panic(expected = "1 yoctoNEAR")]
fn delegating_the_installer_needs_a_full_access_signature() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx(OWNER, 0, 0);
    c.set_installer(AccountId::from_str(INSTALLER).unwrap());
}

#[test]
#[should_panic(expected = "only owner")]
fn set_installer_rejects_a_non_owner() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_yocto("attacker.testnet", 0, 0);
    c.set_installer(AccountId::from_str("attacker.testnet").unwrap());
}

#[test]
#[should_panic(expected = "has not armed a policy install")]
fn the_installer_cannot_install_a_policy_the_account_never_armed() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, 60);
}

#[test]
#[should_panic(expected = "attestation key does not match")]
fn the_installer_cannot_swap_the_attestation_key_the_account_armed() {
    let (_, wk1) = keypair();
    let (_, armed_pk) = keypair();
    let (_, other_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx(VICTIM, 0, 0);
    c.arm_policy_install(armed_pk, 60);
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), other_pk, 60);
}

#[test]
#[should_panic(expected = "timelock does not match")]
fn the_installer_cannot_shorten_the_timelock_the_account_armed() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx(VICTIM, 0, 0);
    c.arm_policy_install(mother_pk.clone(), 259_200);
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, 60);
}

#[test]
#[should_panic(expected = "has not armed a policy install")]
fn an_arming_is_consumed_by_the_install_it_authorised() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    install(&mut c, mother_pk.clone());
    ctx(OWNER, 0, 0);
    c.install_policy(
        AccountId::from_str("other.testnet").unwrap(),
        mpc_public_key(),
        mother_pk,
        60,
    );
}

#[test]
fn an_account_can_withdraw_its_arming_before_the_install() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx(VICTIM, 0, 0);
    c.arm_policy_install(mother_pk, 60);
    assert!(c.armed_policy_install(account_id()).is_some());
    ctx(VICTIM, 0, 0);
    c.disarm_policy_install();
    assert!(c.armed_policy_install(account_id()).is_none());
}

#[test]
fn a_delegated_installer_can_install_a_policy() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_yocto(OWNER, 0, 0);
    c.set_installer(AccountId::from_str(INSTALLER).unwrap());
    ctx(VICTIM, 0, 0);
    c.arm_policy_install(mother_pk.clone(), 259_200);
    ctx(INSTALLER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, 259_200);
    assert_eq!(c.timelock_of(account_id()), Some(259_200));
}

#[test]
fn the_owner_can_still_install_after_delegating() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_yocto(OWNER, 0, 0);
    c.set_installer(AccountId::from_str(INSTALLER).unwrap());
    ctx(VICTIM, 0, 0);
    c.arm_policy_install(mother_pk.clone(), 259_200);
    ctx(OWNER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, 259_200);
    assert_eq!(c.timelock_of(account_id()), Some(259_200));
}

#[test]
#[should_panic(expected = "only installer or owner")]
fn install_rejects_an_unauthorized_caller() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx("attacker.testnet", 0, 0);
    c.install_policy(account_id(), mpc_public_key(), mother_pk, 259_200);
}

#[test]
fn a_delegated_installer_can_finalize_an_approved_recovery() {
    let (w1, wk1) = keypair();
    let (mother, mother_pk) = keypair();
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
    install(&mut c, mother_pk);
    approve_native_recovery(&mut c, &mother, &w1, wk1);
    ctx_yocto(OWNER, 61 * NS_PER_SEC, 6);
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
    let mut c = deploy(&[wk1.clone(), spare_watcher()], 2);
    install(&mut c, mother_pk);
    approve_recovery(&mut c, &mother, &w1, wk1);
    ctx_yocto(OWNER, 0, 6);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    install(&mut c, mother_pk);
    ctx_yocto(OWNER, 0, 0);
    c.set_installer(AccountId::from_str(INSTALLER).unwrap());
    ctx(INSTALLER, 0, 0);
    c.install_policy(account_id(), mpc_public_key(), attacker_pk, 259_200);
}

#[test]
fn the_owner_can_still_replace_an_existing_policy() {
    let (_, wk1) = keypair();
    let (_, mother_pk) = keypair();
    let (_, next_pk) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_paying(OWNER, 0, 1);
    c.approve_upgrade(code_hash());
    ctx_paying(OWNER, UPGRADE_DELAY_NS - 1, 1);
    let _ = c.upgrade(code());
}

#[test]
#[should_panic(expected = "code does not match the approved hash")]
fn upgrade_with_different_code_is_rejected() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_paying(OWNER, 0, 1);
    c.approve_upgrade(code_hash());
    ctx_paying(OWNER, AFTER_UPGRADE_DELAY, 1);
    let _ = c.upgrade(Base64VecU8(vec![8u8; 64]));
}

#[test]
#[should_panic(expected = "no approved code hash")]
fn upgrade_without_an_approval_is_rejected() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_paying(OWNER, AFTER_UPGRADE_DELAY, 1);
    let _ = c.upgrade(code());
}

#[test]
#[should_panic(expected = "only owner")]
fn a_non_owner_cannot_approve_an_upgrade() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_paying("attacker.testnet", 0, 1);
    c.approve_upgrade(code_hash());
}

#[test]
#[should_panic(expected = "requires an attached deposit of exactly 1 yoctoNEAR")]
fn a_restricted_key_cannot_approve_an_upgrade() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_paying(OWNER, 0, 0);
    c.approve_upgrade(code_hash());
}

#[test]
fn the_owner_can_rotate_the_watcher_set() {
    let (_, wk1) = keypair();
    let (_, wk2) = keypair();
    let (_, wk3) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_paying(OWNER, 0, 1);
    c.set_watchers(vec![wk2], 1);
}

#[test]
#[should_panic(expected = "duplicate watcher key")]
fn the_watcher_set_rejects_a_duplicate_key() {
    let (_, wk1) = keypair();
    let (_, wk2) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_paying(OWNER, 0, 1);
    c.set_watchers(vec![wk2.clone(), wk2], 2);
}

#[test]
#[should_panic(expected = "only owner")]
fn a_non_owner_cannot_rotate_the_watcher_set() {
    let (_, wk1) = keypair();
    let (_, wk2) = keypair();
    let (_, wk3) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    ctx_paying("attacker.testnet", 0, 1);
    c.set_watchers(vec![wk2, wk3], 2);
}

#[test]
#[should_panic(expected = "no contract state to migrate")]
fn migrate_refuses_a_shape_it_does_not_recognise() {
    ctx_paying(OWNER, 0, 0);
    env::state_write(&(AccountId::from_str(OWNER).unwrap(), 7u64));
    MpcRecovery::migrate();
}

#[test]
#[should_panic(expected = "state version")]
fn migrate_refuses_a_state_version_it_does_not_understand() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    c.state_version = crate::STATE_VERSION + 1;
    ctx_paying(OWNER, 0, 0);
    env::state_write(&c);
    MpcRecovery::migrate();
}

#[test]
fn migrate_is_idempotent() {
    let (_, wk1) = keypair();
    let c = deploy(&[wk1, spare_watcher()], 2);
    let owner = c.owner.clone();
    env::state_write(&c);
    let once = MpcRecovery::migrate();
    env::state_write(&once);
    assert_eq!(MpcRecovery::migrate().owner, owner);
}

#[test]
fn state_already_current_survives_a_same_shape_redeploy() {
    let (_, wk1) = keypair();
    let c = deploy(&[wk1, spare_watcher()], 2);
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
fn the_owner_can_seal_the_recovery_contract_once_the_upgrade_path_is_proven() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    c.upgrade_proven = true;
    seal_ctx(OWNER, 1);
    let _ = c.seal(mpc_public_key());
}

#[test]
#[should_panic(expected = "upgrade path has not been exercised")]
fn sealing_recovery_is_refused_until_an_upgrade_has_run() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    seal_ctx(OWNER, 1);
    let _ = c.seal(mpc_public_key());
}

#[test]
#[should_panic(expected = "only owner")]
fn an_owner_that_cannot_call_cannot_remove_the_key() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
    seal_ctx("attacker.testnet", 1);
    let _ = c.seal(mpc_public_key());
}

#[test]
#[should_panic]
fn sealing_the_recovery_contract_needs_a_full_access_signature() {
    let (_, wk1) = keypair();
    let mut c = deploy(&[wk1, spare_watcher()], 2);
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
        vec![wk1, spare_watcher()],
        2,
    );
}

mod stored_shape {
    use super::*;
    use crate::state::{Account, Phase, Policy};
    use near_sdk::borsh;

    fn discriminant(phase: Phase) -> u8 {
        let account = Account {
            policy: Policy {
                mpc_public_key: mpc_public_key(),
                attestation_key: mpc_public_key(),
                timelock_secs: 60,
            },
            round: 0,
            phase,
        };
        let account_bytes = borsh::to_vec(&account).expect("Account must serialize");
        let phase_bytes = borsh::to_vec(&account.phase).expect("Phase must serialize");
        assert!(
            account_bytes.ends_with(&phase_bytes),
            "Phase must remain the trailing field of Account"
        );
        phase_bytes[0]
    }

    #[test]
    fn phase_variants_are_append_only() {
        let key = mpc_public_key();
        let name: AccountId = AccountId::from_str("bob.testnet").unwrap();
        assert_eq!(discriminant(Phase::Idle), 0);
        assert_eq!(
            discriminant(Phase::Requested {
                new_owner: key.clone(),
                round: 0,
                requested_at: 0,
            }),
            1
        );
        assert_eq!(
            discriminant(Phase::Approved {
                new_owner: key.clone(),
                round: 0,
            }),
            2,
            "every stored Account carries a Phase discriminant, so reordering or inserting a \
             variant silently reinterprets the recovery state of every account on chain"
        );
        assert_eq!(
            discriminant(Phase::Resolving {
                new_owner: key,
                round: 0,
            }),
            3
        );
        assert_eq!(
            discriminant(Phase::NameRequested {
                new_owner: name.clone(),
                round: 0,
                requested_at: 0,
            }),
            4
        );
        assert_eq!(
            discriminant(Phase::NameResolving {
                new_owner: name,
                round: 0,
                requested_at: 0,
            }),
            5
        );
    }
}

mod state_version {
    use super::*;

    #[test]
    fn new_stamps_the_version_into_state() {
        let (_, wk1) = keypair();
        let c = deploy(&[wk1, spare_watcher()], 2);
        assert_eq!(c.state_version, crate::STATE_VERSION);
    }

    #[test]
    fn the_version_is_the_first_field_so_it_is_readable_before_anything_else() {
        let (_, wk1) = keypair();
        let c = deploy(&[wk1, spare_watcher()], 2);
        let bytes = near_sdk::borsh::to_vec(&c).unwrap();
        assert_eq!(
            u16::from_le_bytes([bytes[0], bytes[1]]),
            crate::STATE_VERSION,
            "a reader must be able to learn the version without parsing the rest"
        );
    }
}
