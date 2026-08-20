mod error;
mod events;
mod proof;
mod state;
mod tx;

use std::collections::BTreeSet;

use near_sdk::json_types::{Base58CryptoHash, Base64VecU8, U64};
use near_sdk::serde_json::{json, Value};
use near_sdk::store::LookupMap;
use near_sdk::{
    env, near, require, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
    PromiseOrValue, PublicKey,
};

use crate::events::Event;
use crate::state::{Account, ArmedPolicy, Phase, Policy};

const SIGN_GAS: Gas = Gas::from_tgas(60);
const CALLBACK_GAS: Gas = Gas::from_tgas(20);
const GAS_FOR_RECOVER_NAME: Gas = Gas::from_tgas(120);
const GAS_FOR_NAME_SETTLE: Gas = Gas::from_tgas(5);
const ED25519_DOMAIN: u64 = 1;
const NS_PER_SEC: u64 = 1_000_000_000;
const MIN_TIMELOCK_SECS: u32 = 60;
const MAX_TIMELOCK_SECS: u32 = 2_592_000;
/// Code changes wait longer than the wallet publish delay, because this
/// contract decides when the MPC signs for someone else's account.
const UPGRADE_DELAY_NS: u64 = 72 * 60 * 60 * 1_000_000_000;
/// Once the watcher set is set through governance it cannot drop to one, so no
/// single watcher can carry a recovery on its own.
const MIN_WATCHER_THRESHOLD: u32 = 2;

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct MpcRecovery {
    owner: AccountId,
    installer: AccountId,
    signer: AccountId,
    transfer_authority: AccountId,
    watchers: Vec<PublicKey>,
    threshold: u32,
    accounts: LookupMap<AccountId, Account>,
    round_floor: LookupMap<AccountId, u64>,
    approved_code_hash: Option<[u8; 32]>,
    approved_at: Option<u64>,
    registry: Option<AccountId>,
    armed: LookupMap<AccountId, ArmedPolicy>,
}

/// The shape currently on chain. Only read by `migrate`, and it has to keep
/// matching the deployed bytes: if it stops, the legacy arm fails and the
/// upgrade silently falls through to whatever the current struct parses as.
#[near(serializers = [borsh])]
struct LegacyMpcRecovery {
    owner: AccountId,
    installer: AccountId,
    signer: AccountId,
    transfer_authority: AccountId,
    watchers: Vec<PublicKey>,
    threshold: u32,
    accounts: LookupMap<AccountId, Account>,
    round_floor: LookupMap<AccountId, u64>,
    approved_code_hash: Option<[u8; 32]>,
    approved_at: Option<u64>,
    registry: Option<AccountId>,
}

#[near(serializers = [json])]
pub struct WatcherSignature {
    pub public_key: PublicKey,
    pub signature: Base64VecU8,
}

#[near(serializers = [json])]
pub enum Verdict {
    Approve,
    Cancel,
}

#[near(serializers = [json])]
pub struct ArmedPolicyView {
    pub attestation_key: PublicKey,
    pub timelock_secs: u32,
}

#[near(serializers = [json])]
pub struct RecoveryResult {
    pub signed_tx_hash: String,
    pub mpc_signature: Value,
}

#[near]
impl MpcRecovery {
    #[init]
    pub fn new(
        owner: AccountId,
        signer: AccountId,
        transfer_authority: AccountId,
        watchers: Vec<PublicKey>,
        threshold: u32,
    ) -> Self {
        require!(threshold >= MIN_WATCHER_THRESHOLD, error::THRESHOLD_TOO_LOW);
        require!((threshold as usize) <= watchers.len(), error::BAD_THRESHOLD);
        require!(owner != env::current_account_id(), error::OWNER_IS_SELF);
        let mut seen = BTreeSet::new();
        for watcher in &watchers {
            require!(hos_common::is_ed25519(watcher), error::WATCHER_NOT_ED25519);
            require!(seen.insert(watcher.clone()), error::DUPLICATE_WATCHER);
        }
        Self {
            installer: owner.clone(),
            owner,
            signer,
            transfer_authority,
            watchers,
            threshold,
            accounts: LookupMap::new(b"a"),
            round_floor: LookupMap::new(b"r"),
            armed: LookupMap::new(b"m"),
            approved_code_hash: None,
            approved_at: None,
            registry: None,
        }
    }

    #[private]
    #[init(ignore_state)]
    pub fn migrate() -> Self {
        Event::Upgraded {}.emit();
        if let Some(current) = hos_common::try_state_read::<Self>() {
            return current;
        }
        let Some(old) = hos_common::try_state_read::<LegacyMpcRecovery>() else {
            env::panic_str(error::NO_STATE)
        };
        Self {
            owner: old.owner,
            installer: old.installer,
            signer: old.signer,
            transfer_authority: old.transfer_authority,
            watchers: old.watchers,
            threshold: old.threshold,
            accounts: old.accounts,
            round_floor: old.round_floor,
            armed: LookupMap::new(b"m"),
            approved_code_hash: None,
            approved_at: None,
            registry: old.registry,
        }
    }

    #[payable]
    pub fn seal(&mut self, public_key: PublicKey) -> Promise {
        self.assert_one_yocto();
        self.assert_owner();
        Event::Sealed {
            public_key: String::from(&public_key),
            by: env::predecessor_account_id(),
        }
        .emit();
        Promise::new(env::current_account_id()).delete_key(public_key)
    }

    #[payable]
    pub fn approve_upgrade(&mut self, code_hash: Base58CryptoHash) {
        self.assert_one_yocto();
        self.assert_owner();
        self.approved_code_hash = Some(code_hash.into());
        self.approved_at = Some(env::block_timestamp());
        Event::UpgradeApproved {
            hash: (&code_hash).into(),
            by: env::predecessor_account_id(),
        }
        .emit();
    }

    #[payable]
    pub fn upgrade(&mut self, code: Base64VecU8) -> Promise {
        self.assert_one_yocto();
        self.assert_owner();
        let code = code.0;
        require!(!code.is_empty(), error::EMPTY_CODE);
        let approved = self
            .approved_code_hash
            .unwrap_or_else(|| env::panic_str(error::NO_APPROVED_HASH));
        require!(env::sha256_array(&code) == approved, error::HASH_MISMATCH);
        let approved_at = self
            .approved_at
            .unwrap_or_else(|| env::panic_str(error::NO_APPROVED_HASH));
        require!(
            env::block_timestamp() >= approved_at.saturating_add(UPGRADE_DELAY_NS),
            error::APPROVAL_TOO_YOUNG
        );
        self.approved_code_hash = None;
        self.approved_at = None;
        let _ = approved;
        hos_common::deploy_and_migrate(code)
    }

    /// Rotating the watcher set is the only way to replace a compromised
    /// watcher without redeploying. The floor of two is what stops any single
    /// watcher, including one of ours, from carrying a recovery alone.
    #[payable]
    pub fn set_watchers(&mut self, watchers: Vec<PublicKey>, threshold: u32) {
        self.assert_one_yocto();
        self.assert_owner();
        require!(threshold >= MIN_WATCHER_THRESHOLD, error::THRESHOLD_TOO_LOW);
        require!((threshold as usize) <= watchers.len(), error::BAD_THRESHOLD);
        let mut seen = BTreeSet::new();
        for watcher in &watchers {
            require!(hos_common::is_ed25519(watcher), error::WATCHER_NOT_ED25519);
            require!(seen.insert(watcher.clone()), error::DUPLICATE_WATCHER);
        }
        let count = watchers.len() as u32;
        self.watchers = watchers;
        self.threshold = threshold;
        Event::WatchersChanged { threshold, count }.emit();
    }

    #[payable]
    pub fn set_installer(&mut self, installer: AccountId) {
        self.assert_one_yocto();
        self.assert_owner();
        self.installer = installer.clone();
        Event::InstallerChanged { installer }.emit();
    }

    #[payable]
    pub fn set_registry(&mut self, registry: AccountId) {
        self.assert_one_yocto();
        self.assert_owner();
        require!(
            registry != env::current_account_id(),
            error::REGISTRY_IS_SELF
        );
        self.registry = Some(registry.clone());
        Event::RegistryChanged { registry }.emit();
    }

    pub fn request_name_recovery(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_owner: AccountId,
        attestation: Base64VecU8,
    ) {
        let contract = env::current_account_id();
        let account = leased_account_id(&tla_id, &name);
        let entry = self
            .accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY));
        require!(matches!(entry.phase, Phase::Idle), error::NOT_IDLE);
        let message =
            proof::name_request_message(&contract, &tla_id, &name, &new_owner, entry.round);
        require!(
            proof::verify(
                &message,
                &into_sig(attestation.into()),
                &entry.policy.attestation_key
            ),
            error::BAD_ATTESTATION
        );
        let round = entry.round;
        entry.phase = Phase::NameRequested {
            new_owner: new_owner.clone(),
            round,
            requested_at: env::block_timestamp(),
        };
        entry.round = round
            .checked_add(1)
            .unwrap_or_else(|| env::panic_str(error::ROUND_EXHAUSTED));
        Event::NameRecoveryRequested {
            tla_id,
            name,
            new_owner,
            round: U64(round),
        }
        .emit();
    }

    #[payable]
    pub fn recover_name(
        &mut self,
        tla_id: AccountId,
        name: String,
        new_owner: AccountId,
        expected_owner: AccountId,
        deadline_ns: U64,
        signatures: Vec<WatcherSignature>,
    ) -> Promise {
        self.assert_one_yocto();
        let registry = self
            .registry
            .clone()
            .unwrap_or_else(|| env::panic_str(error::NO_REGISTRY));
        require!(
            env::block_timestamp() < deadline_ns.0,
            error::QUORUM_EXPIRED
        );
        let account = leased_account_id(&tla_id, &name);
        let entry = self
            .accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY));
        let timelock_ns = u64::from(entry.policy.timelock_secs)
            .checked_mul(1_000_000_000)
            .unwrap_or_else(|| env::panic_str(error::TIMELOCK_TOO_LONG));
        let (pending_owner, round, requested_at) = entry
            .phase
            .name_pending()
            .unwrap_or_else(|| env::panic_str(error::NAME_NOT_REQUESTED));
        require!(*pending_owner == new_owner, error::DESTINATION_CHANGED);
        require!(
            env::block_timestamp()
                >= requested_at
                    .checked_add(timelock_ns)
                    .unwrap_or_else(|| env::panic_str(error::TIMELOCK_TOO_LONG)),
            error::TIMELOCK_ACTIVE
        );
        let message = proof::name_recovery_message(
            &env::current_account_id(),
            &tla_id,
            &name,
            &new_owner,
            &expected_owner,
            deadline_ns.0,
            round,
        );
        let sigs: Vec<(PublicKey, [u8; 64])> = signatures
            .into_iter()
            .filter_map(|w| {
                let bytes: Vec<u8> = w.signature.into();
                <[u8; 64]>::try_from(bytes.as_slice())
                    .ok()
                    .map(|sig| (w.public_key, sig))
            })
            .collect();
        require!(
            proof::verify_quorum(&message, &sigs, &self.watchers, self.threshold),
            error::NO_QUORUM
        );
        self.accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY))
            .phase = Phase::NameResolving {
            new_owner: new_owner.clone(),
            round,
            requested_at,
        };
        Event::NameRecoveryApproved {
            tla_id: tla_id.clone(),
            name: name.clone(),
            new_owner: new_owner.clone(),
        }
        .emit();
        Promise::new(registry)
            .function_call(
                "recover_sub_account".to_string(),
                near_sdk::serde_json::json!({
                    "tla_id": tla_id,
                    "name": name,
                    "new_owner": new_owner,
                    "expected_owner": expected_owner,
                })
                .to_string()
                .into_bytes(),
                NearToken::from_yoctonear(1),
                GAS_FOR_RECOVER_NAME,
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(GAS_FOR_NAME_SETTLE)
                    .on_name_recovered(account, round),
            )
    }

    #[private]
    pub fn on_name_recovered(
        &mut self,
        account: AccountId,
        round: u64,
        #[callback_result] outcome: Result<(), PromiseError>,
    ) -> bool {
        let settled = outcome.is_ok();
        self.settle_name_resolving(&account, round, settled);
        settled
    }

    pub fn arm_policy_install(&mut self, attestation_key: PublicKey, timelock_secs: u32) {
        let account = env::predecessor_account_id();
        require!(
            hos_common::is_ed25519(&attestation_key),
            error::ATTESTATION_NOT_ED25519
        );
        require!(
            timelock_secs >= MIN_TIMELOCK_SECS,
            error::TIMELOCK_TOO_SHORT
        );
        require!(timelock_secs <= MAX_TIMELOCK_SECS, error::TIMELOCK_TOO_LONG);
        self.armed.insert(
            account.clone(),
            ArmedPolicy {
                attestation_key: attestation_key.clone(),
                timelock_secs,
            },
        );
        Event::PolicyInstallArmed {
            account,
            attestation_key,
            timelock_secs,
        }
        .emit();
    }

    pub fn disarm_policy_install(&mut self) {
        let account = env::predecessor_account_id();
        require!(self.armed.remove(&account).is_some(), error::NOT_ARMED);
        Event::PolicyInstallDisarmed { account }.emit();
    }

    pub fn armed_policy_install(&self, account: AccountId) -> Option<ArmedPolicyView> {
        self.armed.get(&account).map(|armed| ArmedPolicyView {
            attestation_key: armed.attestation_key.clone(),
            timelock_secs: armed.timelock_secs,
        })
    }

    pub fn install_policy(
        &mut self,
        account: AccountId,
        mpc_public_key: PublicKey,
        attestation_key: PublicKey,
        timelock_secs: u32,
    ) {
        require!(self.is_installer(), error::ONLY_INSTALLER);
        require!(
            timelock_secs >= MIN_TIMELOCK_SECS,
            error::TIMELOCK_TOO_SHORT
        );
        require!(timelock_secs <= MAX_TIMELOCK_SECS, error::TIMELOCK_TOO_LONG);
        require!(
            hos_common::is_ed25519(&attestation_key),
            error::ATTESTATION_NOT_ED25519
        );
        require!(
            hos_common::is_ed25519(&mpc_public_key),
            error::MPC_NOT_ED25519
        );
        let round = match self.accounts.get(&account) {
            Some(existing) => {
                require!(matches!(existing.phase, Phase::Idle), error::NOT_IDLE);
                require!(
                    env::predecessor_account_id() == self.owner,
                    error::ONLY_OWNER_REINSTALL
                );
                existing.round
            }
            None => {
                let armed = self
                    .armed
                    .remove(&account)
                    .unwrap_or_else(|| env::panic_str(error::NOT_ARMED));
                require!(
                    armed.attestation_key == attestation_key,
                    error::ARMED_KEY_MISMATCH
                );
                require!(
                    armed.timelock_secs == timelock_secs,
                    error::ARMED_TIMELOCK_MISMATCH
                );
                self.round_floor.get(&account).copied().unwrap_or(0)
            }
        };
        self.accounts.insert(
            account.clone(),
            Account {
                policy: Policy {
                    mpc_public_key: mpc_public_key.clone(),
                    attestation_key: attestation_key.clone(),
                    timelock_secs,
                },
                round,
                phase: Phase::Idle,
            },
        );
        Event::PolicyInstalled {
            account,
            timelock_secs,
            mpc_public_key,
            attestation_key,
        }
        .emit();
    }

    pub fn request_recovery(
        &mut self,
        account: AccountId,
        new_owner: PublicKey,
        round: U64,
        attestation: Base64VecU8,
    ) {
        let contract = env::current_account_id();
        let entry = self
            .accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY));
        require!(matches!(entry.phase, Phase::Idle), error::NOT_IDLE);
        require!(round.0 == entry.round, error::STALE_ROUND);
        let message = proof::request_message(&contract, &account, &new_owner, entry.round);
        require!(
            proof::verify(
                &message,
                &into_sig(attestation.into()),
                &entry.policy.attestation_key
            ),
            error::BAD_ATTESTATION
        );
        let round = entry.round;
        entry.phase = Phase::Requested {
            new_owner: new_owner.clone(),
            round,
            requested_at: env::block_timestamp(),
        };
        entry.round = round
            .checked_add(1)
            .unwrap_or_else(|| env::panic_str(error::ROUND_EXHAUSTED));
        Event::Requested {
            account,
            round: U64(round),
            new_owner,
        }
        .emit();
    }

    pub fn submit_verdict(
        &mut self,
        account: AccountId,
        verdict: Verdict,
        signatures: Vec<WatcherSignature>,
    ) -> PromiseOrValue<()> {
        let contract = env::current_account_id();
        let watchers = self.watchers.clone();
        let threshold = self.threshold;
        let entry = self
            .accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY));
        let (new_owner, round, requested_at) = match &entry.phase {
            Phase::Requested {
                new_owner,
                round,
                requested_at,
            } => (new_owner.clone(), *round, *requested_at),
            _ => env::panic_str(error::NOT_REQUESTED),
        };
        require!(
            env::block_timestamp()
                >= requested_at + (entry.policy.timelock_secs as u64) * NS_PER_SEC,
            error::TIMELOCK
        );
        let approve = matches!(verdict, Verdict::Approve);
        let message = proof::verdict_message(&contract, &account, &new_owner, round, approve);
        let sigs: Vec<(PublicKey, [u8; 64])> = signatures
            .into_iter()
            .filter_map(|w| {
                let bytes: Vec<u8> = w.signature.into();
                <[u8; 64]>::try_from(bytes.as_slice())
                    .ok()
                    .map(|sig| (w.public_key, sig))
            })
            .collect();
        require!(
            proof::verify_quorum(&message, &sigs, &watchers, threshold),
            error::NO_QUORUM
        );
        if !approve {
            entry.phase = Phase::Idle;
            Event::Canceled {
                account,
                round: U64(round),
            }
            .emit();
            return PromiseOrValue::Value(());
        }
        entry.phase = Phase::Approved { new_owner, round };
        Event::Approved {
            account,
            round: U64(round),
        }
        .emit();
        PromiseOrValue::Value(())
    }

    pub fn finalize_recovery(
        &mut self,
        account: AccountId,
        nonce: U64,
        block_hash: Base58CryptoHash,
    ) -> Promise {
        let authorized = self.is_installer();
        let entry = self
            .accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY));
        let (new_owner, round) = match &entry.phase {
            Phase::Approved { new_owner, round } => (new_owner.clone(), *round),
            _ => env::panic_str(error::NOT_APPROVED),
        };
        require!(authorized, error::ONLY_INSTALLER);
        let mpc_public_key = entry.policy.mpc_public_key.clone();
        entry.phase = Phase::Resolving {
            new_owner: new_owner.clone(),
            round,
        };
        self.sign_add_key(&AddKeyRequest {
            account,
            mpc_public_key,
            nonce: nonce.0,
            block_hash: block_hash.into(),
            new_owner,
            round,
        })
    }

    pub fn abort_recovery(&mut self, account: AccountId) -> PromiseOrValue<()> {
        require!(
            self.is_installer() || env::predecessor_account_id() == account,
            error::ONLY_INSTALLER_OR_HOLDER
        );
        let entry = self
            .accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY));
        let round = match &entry.phase {
            Phase::Requested { round, .. }
            | Phase::Approved { round, .. }
            | Phase::NameRequested { round, .. } => *round,
            Phase::Idle | Phase::Resolving { .. } | Phase::NameResolving { .. } => {
                env::panic_str(error::NOT_ACTIVE)
            }
        };
        entry.phase = Phase::Idle;
        Event::Aborted {
            account,
            round: U64(round),
        }
        .emit();
        PromiseOrValue::Value(())
    }

    #[private]
    pub fn on_signed(
        &mut self,
        account: AccountId,
        round: u64,
        signed_tx_hash: String,
        #[callback_result] mpc_signature: Result<Value, PromiseError>,
    ) -> Option<RecoveryResult> {
        let reverted = self.settle_resolving(&account, round, false);
        match mpc_signature {
            Ok(mpc_signature) if reverted => {
                Event::NativeSignatureProduced {
                    account,
                    round: U64(round),
                }
                .emit();
                Some(RecoveryResult {
                    signed_tx_hash,
                    mpc_signature,
                })
            }
            _ => None,
        }
    }

    pub fn claim_native_finalized(&mut self, account: AccountId, round: U64) {
        require!(self.is_installer(), error::ONLY_INSTALLER);
        let entry = self
            .accounts
            .get_mut(&account)
            .unwrap_or_else(|| env::panic_str(error::NO_POLICY));
        match &entry.phase {
            Phase::Approved {
                round: approved, ..
            } if *approved == round.0 => {}
            _ => env::panic_str(error::NOT_APPROVED),
        }
        entry.phase = Phase::Idle;
        Event::Finalized { account, round }.emit();
    }

    pub fn round_of(&self, account: AccountId) -> Option<u64> {
        self.accounts.get(&account).map(|a| a.round)
    }

    pub fn timelock_of(&self, account: AccountId) -> Option<u32> {
        self.accounts.get(&account).map(|a| a.policy.timelock_secs)
    }

    pub fn pending_target(&self, account: AccountId) -> Option<String> {
        self.accounts
            .get(&account)
            .and_then(|a| a.phase.pending().map(|(key, _)| key.to_string()))
    }

    pub fn expected_native_path(&self, account: AccountId) -> String {
        native_path(&account)
    }

    pub fn owner(&self) -> AccountId {
        self.owner.clone()
    }

    pub fn installer(&self) -> AccountId {
        self.installer.clone()
    }

    pub fn signer(&self) -> AccountId {
        self.signer.clone()
    }

    pub fn transfer_authority(&self) -> AccountId {
        self.transfer_authority.clone()
    }

    pub fn registry(&self) -> Option<AccountId> {
        self.registry.clone()
    }

    pub fn watchers(&self) -> Vec<PublicKey> {
        self.watchers.clone()
    }

    pub fn threshold(&self) -> u32 {
        self.threshold
    }

    pub fn approved_upgrade_hash(&self) -> Option<Base58CryptoHash> {
        self.approved_code_hash.map(Base58CryptoHash::from)
    }

    pub fn approved_upgrade_at(&self) -> Option<U64> {
        self.approved_at.map(U64)
    }

    pub fn upgrade_delay_ns(&self) -> U64 {
        U64(UPGRADE_DELAY_NS)
    }

    pub fn on_wallet_transferred(&mut self, wallet: AccountId) -> bool {
        require!(
            env::predecessor_account_id() == self.transfer_authority,
            error::ONLY_TRANSFER_AUTHORITY
        );
        let Some(account) = self.accounts.get(&wallet) else {
            return true;
        };
        if matches!(account.phase, Phase::Resolving { .. }) {
            Event::PolicyResetDeferred {
                account: wallet,
                round: U64(account.round),
            }
            .emit();
            return false;
        }
        let round = account.round;
        self.round_floor.insert(wallet.clone(), round);
        self.accounts.remove(&wallet);
        Event::PolicyReset { account: wallet }.emit();
        true
    }

    fn is_installer(&self) -> bool {
        let caller = env::predecessor_account_id();
        caller == self.installer || caller == self.owner
    }

    fn assert_owner(&self) {
        require!(
            env::predecessor_account_id() == self.owner,
            error::ONLY_OWNER
        );
    }

    fn assert_one_yocto(&self) {
        require!(
            env::attached_deposit() == NearToken::from_yoctonear(1),
            error::REQUIRES_ONE_YOCTO
        );
    }

    fn sign_add_key(&self, req: &AddKeyRequest) -> Promise {
        let AddKeyRequest {
            account,
            mpc_public_key,
            nonce,
            block_hash,
            new_owner,
            round,
        } = req;
        let path = native_path(account);
        let unsigned = tx::add_key_tx(account, mpc_public_key, *nonce, block_hash, new_owner);
        let hash_hex = tx::to_hex(&env::sha256(&unsigned));
        let args = json!({"request": {"path": path, "payload_v2": {"Eddsa": hash_hex}, "domain_id": ED25519_DOMAIN}})
            .to_string()
            .into_bytes();
        Promise::new(self.signer.clone())
            .function_call(
                "sign".to_string(),
                args,
                NearToken::from_yoctonear(1),
                SIGN_GAS,
            )
            .then(
                Self::ext(env::current_account_id())
                    .with_static_gas(CALLBACK_GAS)
                    .on_signed(account.clone(), *round, hash_hex),
            )
    }

    fn settle_name_resolving(&mut self, account: &AccountId, round: u64, done: bool) -> bool {
        let Some(entry) = self.accounts.get_mut(account) else {
            return false;
        };
        let Some((new_owner, requested_at)) = entry.phase.name_resolving(round) else {
            return false;
        };
        entry.phase = if done {
            Phase::Idle
        } else {
            Phase::NameRequested {
                new_owner,
                round,
                requested_at,
            }
        };
        true
    }

    fn settle_resolving(&mut self, account: &AccountId, round: u64, done: bool) -> bool {
        let Some(entry) = self.accounts.get_mut(account) else {
            return false;
        };
        let Some(new_owner) = entry.phase.resolving_owner(round) else {
            return false;
        };
        entry.phase = if done {
            Phase::Idle
        } else {
            Phase::Approved { new_owner, round }
        };
        true
    }
}

struct AddKeyRequest {
    account: AccountId,
    mpc_public_key: PublicKey,
    nonce: u64,
    block_hash: [u8; 32],
    new_owner: PublicKey,
    round: u64,
}

fn native_path(account: &AccountId) -> String {
    format!("hos-recovery/{account}")
}

fn leased_account_id(tla_id: &AccountId, name: &str) -> AccountId {
    format!("{name}.{tla_id}")
        .parse()
        .unwrap_or_else(|_| env::panic_str(error::INVALID_LEASED_ACCOUNT))
}

fn into_sig(bytes: Vec<u8>) -> [u8; 64] {
    <[u8; 64]>::try_from(bytes.as_slice())
        .unwrap_or_else(|_| env::panic_str(error::BAD_SIGNATURE_LEN))
}

#[cfg(test)]
mod tests;
