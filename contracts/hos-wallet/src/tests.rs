use super::*;
use defuse_wallet::actions::FunctionCall;
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::testing_env;

const WALLET: &str = "alice.tla.testnet";
const PARENT: &str = "tla.testnet";
const AUTHORITY: &str = "hos-extension.testnet";
const PAYOUT: &str = "payout.testnet";
const OWNER: &str = "renter.testnet";
const BUYER: &str = "buyer.testnet";
const YEAR_NS: u64 = 31_536_000_000_000_000;

fn acc(s: &str) -> AccountId {
    s.parse().unwrap()
}

fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        * 1_000_000_000
}

fn ctx(predecessor: &str, deposit: u128, ts: u64) {
    ctx_bal(predecessor, deposit, ts, NearToken::from_near(10));
}

fn ctx_bal(predecessor: &str, deposit: u128, ts: u64, balance: NearToken) {
    let mut b = VMContextBuilder::new();
    b.current_account_id(acc(WALLET))
        .predecessor_account_id(acc(predecessor))
        .attached_deposit(NearToken::from_yoctonear(deposit))
        .account_balance(balance)
        .block_timestamp(ts);
    testing_env!(b.build());
}

fn init(ts: u64, lease_until_ns: u64) -> TenantWallet {
    ctx(PARENT, 0, ts);
    TenantWallet::hos_init(WalletInit {
        owner_account: acc(OWNER),
        authority: acc(AUTHORITY),
        payout_account: acc(PAYOUT),
        lease_until_ns: U64(lease_until_ns),
        timeout_secs: 3600,
    })
}

fn deploy() -> TenantWallet {
    init(now_ns(), now_ns() + YEAR_NS)
}

fn request(ops: impl IntoIterator<Item = WalletOp>) -> Request {
    Request::new().internal(ops)
}

fn add_co_owner(to: &str) -> WalletOp {
    WalletOp::AddExtension {
        account_id: acc(to),
    }
}

fn send(to: &str, amount: NearToken) -> NearPromise {
    NearPromise::new(acc(to)).transfer(amount)
}

fn auth_blob(message: &str) -> String {
    near_sdk::serde_json::json!({ "message": message }).to_string()
}

#[test]
fn a_wallet_is_owned_by_an_account_and_carries_no_key() {
    let c = deploy();
    assert!(!c.w_is_signature_allowed());
    assert_eq!(c.w_public_key(), "");
    assert!(c.w_is_extension_enabled(acc(OWNER)));
    assert!(c.w_is_extension_enabled(acc(AUTHORITY)));
    assert_eq!(c.w_extensions().len(), 2);
    assert_eq!(c.hos_lease().frozen, FreezeState::Unfrozen);
}

#[test]
#[should_panic(expected = "signature is disabled")]
fn a_signed_request_is_always_refused() {
    let mut c = deploy();
    ctx("relayer.testnet", 1, now_ns());
    let msg = RequestMessage {
        pay_for_gas: false,
        chain_id: env::chain_id(),
        signer_id: acc(WALLET),
        nonce: 1,
        created_at: Timestamp::from_secs(0).unwrap(),
        timeout: Duration::from_secs(3600),
        request: Request::new(),
    };
    c.w_execute_signed(msg, "ed25519:whatever".to_string());
}

#[test]
#[should_panic(expected = "signature is disabled")]
fn signature_mode_can_never_be_switched_on() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([WalletOp::SetSignatureMode { enable: true }]));
}

#[test]
#[should_panic(expected = "signature is disabled")]
fn signature_mode_cannot_be_touched_by_the_authority_either() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.w_execute_extension(request([WalletOp::SetSignatureMode { enable: false }]));
}

#[test]
fn the_owner_acts_through_the_extension_path() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    assert!(c.w_is_extension_enabled(acc(BUYER)));
}

#[test]
#[should_panic(expected = "is not enabled")]
fn a_stranger_account_cannot_execute_as_an_extension() {
    let mut c = deploy();
    ctx("attacker.testnet", 1, now_ns());
    c.w_execute_extension(request([]));
}

#[test]
#[should_panic(expected = "insufficient attached deposit")]
fn the_extension_path_requires_a_non_zero_deposit() {
    let mut c = deploy();
    ctx(OWNER, 0, now_ns());
    c.w_execute_extension(request([]));
}

#[test]
#[should_panic(expected = "the lease authority extension cannot be removed")]
fn the_renter_cannot_evict_the_lease_authority() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([WalletOp::RemoveExtension {
        account_id: acc(AUTHORITY),
    }]));
}

#[test]
#[should_panic(expected = "the lease authority extension cannot be removed")]
fn not_even_the_authority_can_evict_itself() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.w_execute_extension(request([WalletOp::RemoveExtension {
        account_id: acc(AUTHORITY),
    }]));
}

#[test]
fn an_owner_may_add_a_co_owner_who_can_then_act() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner("carol.testnet")]));
    assert!(c.w_is_extension_enabled(acc("carol.testnet")));
}

#[test]
fn the_authority_can_still_act_after_the_lease_expires() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns() + YEAR_NS + 1);
    c.w_execute_extension(request([WalletOp::RemoveExtension {
        account_id: acc(OWNER),
    }]));
    assert!(!c.w_is_extension_enabled(acc(OWNER)));
}

#[test]
#[should_panic(expected = "lease expired")]
fn the_renter_cannot_act_after_the_lease_expires() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns() + YEAR_NS + 1);
    c.w_execute_extension(request([add_co_owner(BUYER)]));
}

#[test]
#[should_panic(expected = "only the lease authority")]
fn the_renter_cannot_extend_its_own_lease() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.hos_set_lease(U64(now_ns() + YEAR_NS * 10), OperatingState::Active);
}

#[test]
fn the_authority_sets_the_lease() {
    let mut c = deploy();
    let target = now_ns() + YEAR_NS * 2;
    ctx(AUTHORITY, 1, now_ns());
    c.hos_set_lease(U64(target), OperatingState::Active);
    assert_eq!(c.hos_lease().lease_until_ns.0, target);
}

#[test]
#[should_panic(expected = "may not move backwards")]
fn the_lease_cannot_move_backwards() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_set_lease(U64(now_ns() + 1), OperatingState::Active);
}

#[test]
#[should_panic(expected = "not settable through lease push")]
fn the_lease_push_cannot_park_the_wallet() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_set_lease(U64(now_ns() + YEAR_NS * 2), OperatingState::Parked);
}

#[test]
fn a_transfer_moves_ownership_and_payout_together() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)));
    assert!(c.w_is_extension_enabled(acc(BUYER)));
    assert!(!c.w_is_extension_enabled(acc(OWNER)));
    assert_eq!(c.hos_payout_account(), acc(BUYER));
}

#[test]
fn a_transfer_evicts_co_owners() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner("carol.testnet")]));
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)));
    assert!(!c.w_is_extension_enabled(acc("carol.testnet")));
    assert!(!c.w_is_extension_enabled(acc(OWNER)));
}

#[test]
fn parking_leaves_only_the_authority_and_keeps_the_payout_account() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(None);
    assert_eq!(c.w_extensions().len(), 1);
    assert!(c.w_is_extension_enabled(acc(AUTHORITY)));
    assert_eq!(c.hos_payout_account(), acc(PAYOUT));
}

#[test]
#[should_panic(expected = "only the lease authority")]
fn the_renter_cannot_transfer_ownership() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)));
}

#[test]
fn the_renter_can_self_freeze_and_unfreeze() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.hos_freeze();
    assert_eq!(c.hos_lease().frozen, FreezeState::SelfFrozen);
    ctx(OWNER, 1, now_ns());
    c.hos_unfreeze();
    assert_eq!(c.hos_lease().frozen, FreezeState::Unfrozen);
}

#[test]
#[should_panic(expected = "only the renter may unfreeze")]
fn the_authority_cannot_unfreeze_a_self_frozen_wallet() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.hos_freeze();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_unfreeze();
}

#[test]
#[should_panic(expected = "only the authority may unfreeze")]
fn the_renter_cannot_unfreeze_an_authority_frozen_wallet() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_freeze();
    ctx(OWNER, 1, now_ns());
    c.hos_unfreeze();
}

#[test]
#[should_panic(expected = "wallet frozen")]
fn a_frozen_wallet_blocks_renter_actions() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_freeze();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
}

#[test]
#[should_panic(expected = "sweep requires an expired lease")]
fn sweeps_require_an_expired_lease() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    let _ = c.hos_sweep_near();
}

#[test]
#[should_panic(expected = "only the lease authority")]
fn the_renter_cannot_sweep() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns() + YEAR_NS + 1);
    let _ = c.hos_sweep_near();
}

#[test]
fn the_authority_sweeps_near_after_expiry() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns() + YEAR_NS + 1);
    let _ = c.hos_sweep_near();
}

#[test]
#[should_panic(expected = "nothing to sweep")]
fn a_sweep_below_the_reserve_is_refused() {
    let mut c = deploy();
    ctx_bal(
        AUTHORITY,
        1,
        now_ns() + YEAR_NS + 1,
        NearToken::from_yoctonear(1),
    );
    let _ = c.hos_sweep_near();
}

#[test]
fn outbound_spending_is_allowed_within_the_reserve() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(Request::new().external([send(BUYER, NearToken::from_near(1))]));
}

#[test]
#[should_panic(expected = "breach the balance reserve")]
fn outbound_spending_cannot_breach_the_balance_reserve() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(Request::new().external([send(BUYER, NearToken::from_near(10))]));
}

#[test]
#[should_panic(expected = "only the renter")]
fn the_authority_cannot_spend_through_the_promise_path() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.w_execute_extension(Request::new().external([send(BUYER, NearToken::from_near(1))]));
}

#[test]
#[should_panic(expected = "self-calls are not allowed")]
fn the_owner_cannot_make_the_wallet_call_itself() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(Request::new().external([send(WALLET, NearToken::from_yoctonear(1))]));
}

#[test]
fn a_function_call_can_be_carried_in_an_external_promise() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    let promise = NearPromise::new(acc("pool.testnet")).function_call(
        FunctionCall::name("deposit_and_stake").attach_deposit(NearToken::from_near(1)),
    );
    c.w_execute_extension(Request::new().external([promise]));
}

#[test]
#[should_panic(expected = "init caller must be the direct parent account")]
fn init_rejects_a_non_parent_caller() {
    ctx("attacker.testnet", 0, now_ns());
    TenantWallet::hos_init(WalletInit {
        owner_account: acc(OWNER),
        authority: acc(AUTHORITY),
        payout_account: acc(PAYOUT),
        lease_until_ns: U64(now_ns() + YEAR_NS),
        timeout_secs: 3600,
    });
}

#[test]
#[should_panic(expected = "lease_until_ns must be in the future")]
fn init_rejects_a_lease_in_the_past() {
    init(now_ns(), now_ns());
}

#[test]
#[should_panic(expected = "unauthorized")]
fn init_rejects_an_owner_that_is_also_the_authority() {
    ctx(PARENT, 0, now_ns());
    TenantWallet::hos_init(WalletInit {
        owner_account: acc(AUTHORITY),
        authority: acc(AUTHORITY),
        payout_account: acc(PAYOUT),
        lease_until_ns: U64(now_ns() + YEAR_NS),
        timeout_secs: 3600,
    });
}

#[test]
fn resolve_auth_always_delegates_to_the_owner_account() {
    let c = deploy();
    match c.w_resolve_auth(
        "PROVE_OWNERSHIP".to_string(),
        "app.example.com".to_string(),
        auth_blob("login"),
    ) {
        AuthorizationResolution::Pending {
            payload,
            pending_authorizations,
        } => {
            assert_eq!(payload, "login");
            assert_eq!(pending_authorizations.len(), 1);
            assert_eq!(pending_authorizations[0].account_id, acc(OWNER));
            assert_eq!(pending_authorizations[0].purpose, "PROVE_OWNERSHIP");
        }
        other => panic!("expected delegation to the owner account, got {other:?}"),
    }
}

#[test]
fn resolve_auth_delegates_to_every_co_owner() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    let AuthorizationResolution::Pending {
        pending_authorizations,
        ..
    } = c.w_resolve_auth(
        "PROVE_OWNERSHIP".to_string(),
        "app.example.com".to_string(),
        auth_blob("login"),
    )
    else {
        panic!("expected delegation");
    };
    let mut owners: Vec<_> = pending_authorizations
        .iter()
        .map(|p| p.account_id.to_string())
        .collect();
    owners.sort();
    assert_eq!(owners, vec![BUYER.to_string(), OWNER.to_string()]);
}

#[test]
fn resolve_auth_never_names_the_authority_as_an_owner() {
    let c = deploy();
    let AuthorizationResolution::Pending {
        pending_authorizations,
        ..
    } = c.w_resolve_auth(
        "PROVE_OWNERSHIP".to_string(),
        "app.example.com".to_string(),
        auth_blob("login"),
    )
    else {
        panic!("expected delegation");
    };
    assert!(pending_authorizations
        .iter()
        .all(|p| p.account_id != acc(AUTHORITY)));
}

#[test]
fn resolve_auth_is_invalid_once_the_account_is_parked() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(None);
    assert!(matches!(
        c.w_resolve_auth(
            "PROVE_OWNERSHIP".to_string(),
            "app.example.com".to_string(),
            auth_blob("login"),
        ),
        AuthorizationResolution::Invalid {
            error_kind: ErrorKind::InvalidSignature,
            ..
        }
    ));
}

#[test]
fn resolve_auth_rejects_malformed_input_without_panicking() {
    let c = deploy();
    assert!(matches!(
        c.w_resolve_auth(
            "PROVE_OWNERSHIP".to_string(),
            "app.example.com".to_string(),
            "not json".to_string(),
        ),
        AuthorizationResolution::Invalid {
            error_kind: ErrorKind::InvalidInput,
            ..
        }
    ));
}

#[test]
fn resolve_auth_serialises_to_the_nep641_wire_format() {
    let c = deploy();
    let pending = near_sdk::serde_json::to_value(c.w_resolve_auth(
        "PROVE_OWNERSHIP".to_string(),
        "app.example.com".to_string(),
        auth_blob("login"),
    ))
    .unwrap();
    assert_eq!(pending["status"], "PENDING");
    assert_eq!(pending["payload"], "login");
    assert_eq!(pending["pending_authorizations"][0]["account_id"], OWNER);

    let bad = near_sdk::serde_json::to_value(c.w_resolve_auth(
        "PROVE_OWNERSHIP".to_string(),
        "app.example.com".to_string(),
        "not json".to_string(),
    ))
    .unwrap();
    assert_eq!(bad["status"], "INVALID");
    assert_eq!(bad["error_kind"], "INVALID_INPUT");
}
