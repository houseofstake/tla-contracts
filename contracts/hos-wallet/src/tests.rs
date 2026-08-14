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
const HOUR_NS: u64 = 3_600_000_000_000;

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

fn arm(c: &mut TenantWallet) {
    ctx(OWNER, 1, now_ns());
    c.hos_arm_transfer(true);
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
    auth_blob_for(message, OWNER)
}

fn auth_blob_for(message: &str, owner: &str) -> String {
    near_sdk::serde_json::json!({
        "payload": message,
        "owner": owner,
        "authorization": "owner-blob",
    })
    .to_string()
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
    ctx(AUTHORITY, 1, now_ns() + YEAR_NS + 1);
    c.w_execute_extension(request([WalletOp::RemoveExtension {
        account_id: acc(AUTHORITY),
    }]));
}

#[test]
#[should_panic(expected = "the authority may edit extensions only after the lease ends")]
fn the_authority_cannot_evict_the_renter_while_the_lease_runs() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.w_execute_extension(request([WalletOp::RemoveExtension {
        account_id: acc(OWNER),
    }]));
}

#[test]
#[should_panic(expected = "the authority may edit extensions only after the lease ends")]
fn the_authority_cannot_add_an_extension_while_the_lease_runs() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
}

fn install_and_grant(c: &mut TenantWallet, cap: NearToken, receivers: &[&str]) {
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    ctx(OWNER, 1, now_ns());
    c.hos_grant_spend(
        acc(BUYER),
        receivers.iter().map(|r| acc(r)).collect(),
        U128(cap.as_yoctonear()),
        U64(now_ns() + HOUR_NS),
    );
}

#[test]
#[should_panic(expected = "no spend grant")]
fn an_installed_extension_cannot_spend_without_a_grant() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(
        Request::new().external([send("carol.testnet", NearToken::from_millinear(1))]),
    );
}

#[test]
fn a_granted_extension_may_spend_within_its_scope() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(10), &["carol.testnet"]);
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(
        Request::new().external([send("carol.testnet", NearToken::from_millinear(1))]),
    );
}

#[test]
#[should_panic(expected = "receiver is not in the spend grant")]
fn a_granted_extension_cannot_pay_an_ungranted_receiver() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(10), &["carol.testnet"]);
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(
        Request::new().external([send("attacker.testnet", NearToken::from_millinear(1))]),
    );
}

#[test]
#[should_panic(expected = "cannot redirect refunds")]
fn a_granted_extension_cannot_redirect_refunds_past_the_allowlist() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(10), &["carol.testnet"]);
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([
        send("carol.testnet", NearToken::from_millinear(1)).refund_to(acc("attacker.testnet")),
    ]));
}

#[test]
fn the_owner_may_still_direct_refunds() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(Request::new().external([
        send("carol.testnet", NearToken::from_millinear(1)).refund_to(acc("carol.testnet")),
    ]));
}

#[test]
#[should_panic(expected = "exceeds the granted cap")]
fn a_granted_extension_cannot_exceed_the_cap() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(1), &["carol.testnet"]);
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(
        Request::new().external([send("carol.testnet", NearToken::from_millinear(2))]),
    );
}

#[test]
#[should_panic(expected = "spend grant expired")]
fn a_grant_stops_working_once_it_expires() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(10), &["carol.testnet"]);
    ctx(BUYER, 1, now_ns() + 2 * HOUR_NS);
    c.w_execute_extension(
        Request::new().external([send("carol.testnet", NearToken::from_millinear(1))]),
    );
}

#[test]
#[should_panic(expected = "no spend grant")]
fn the_owner_can_revoke_a_grant() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(10), &["carol.testnet"]);
    ctx(OWNER, 1, now_ns());
    c.hos_revoke_spend(acc(BUYER));
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(
        Request::new().external([send("carol.testnet", NearToken::from_millinear(1))]),
    );
}

#[test]
#[should_panic(expected = "only the owner")]
fn an_extension_cannot_grant_itself_spend() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(1), &["carol.testnet"]);
    ctx(BUYER, 1, now_ns());
    c.hos_grant_spend(
        acc(BUYER),
        vec![acc("attacker.testnet")],
        U128(NearToken::from_near(5).as_yoctonear()),
        U64(now_ns() + YEAR_NS),
    );
}

#[test]
fn evicting_an_extension_drops_its_grant() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(10), &["carol.testnet"]);
    assert!(c.hos_spend_grant(acc(BUYER)).is_some());
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([WalletOp::RemoveExtension {
        account_id: acc(BUYER),
    }]));
    assert!(c.hos_spend_grant(acc(BUYER)).is_none());
}

#[test]
fn a_sale_clears_every_grant_the_previous_owner_made() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(10), &["carol.testnet"]);
    arm(&mut c);
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc("newowner.testnet")), RotationCause::Sale);
    assert!(
        c.hos_spend_grant(acc(BUYER)).is_none(),
        "a buyer must not inherit the seller's spend grants"
    );
}

#[test]
#[should_panic(expected = "only the owner")]
fn an_installed_extension_cannot_add_another() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner("carol.testnet")]));
}

#[test]
#[should_panic(expected = "only the owner")]
fn an_installed_extension_cannot_evict_the_owner() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(request([WalletOp::RemoveExtension {
        account_id: acc(OWNER),
    }]));
}

#[test]
#[should_panic(expected = "only the owner")]
fn an_installed_extension_cannot_arm_a_transfer() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    ctx(BUYER, 1, now_ns());
    c.hos_arm_transfer(true);
}

#[test]
#[should_panic(expected = "unauthorized")]
fn an_installed_extension_cannot_freeze_the_account() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    ctx(BUYER, 1, now_ns());
    c.hos_freeze();
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
    arm(&mut c);
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer);
    assert!(c.w_is_extension_enabled(acc(BUYER)));
    assert!(!c.w_is_extension_enabled(acc(OWNER)));
    assert_eq!(c.hos_payout_account(), acc(BUYER));
}

#[test]
fn a_transfer_evicts_co_owners() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner("carol.testnet")]));
    arm(&mut c);
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer);
    assert!(!c.w_is_extension_enabled(acc("carol.testnet")));
    assert!(!c.w_is_extension_enabled(acc(OWNER)));
}

#[test]
fn parking_leaves_only_the_authority_and_keeps_the_payout_account() {
    let mut c = deploy();
    arm(&mut c);
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(None, RotationCause::Reclaim);
    assert_eq!(c.w_extensions().len(), 1);
    assert!(c.w_is_extension_enabled(acc(AUTHORITY)));
    assert_eq!(c.hos_payout_account(), acc(PAYOUT));
}

#[test]
#[should_panic(expected = "only the lease authority")]
fn the_renter_cannot_transfer_ownership() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer);
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
fn resolve_auth_delegates_to_the_named_owner() {
    let c = deploy();
    let out = c.w_resolve_auth(vec![], auth_blob("login"));
    assert_eq!(out.payload, "login");
    assert_eq!(out.pending.len(), 1);
    assert_eq!(out.pending[0].account_id, acc(OWNER));
    assert_eq!(out.pending[0].authorization, "owner-blob");
}

/// Every entry in `pending` has to resolve, so naming both owners would mean a
/// co-owner had to sign before the renter could log in.
#[test]
fn resolve_auth_names_one_owner_even_when_a_co_owner_exists() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    let out = c.w_resolve_auth(vec![], auth_blob_for("login", BUYER));
    assert_eq!(out.pending.len(), 1);
    assert_eq!(out.pending[0].account_id, acc(BUYER));
}

#[test]
fn resolve_auth_binds_the_expected_payload_to_the_edge() {
    let c = deploy();
    let out = c.w_resolve_auth(vec![], auth_blob("login"));
    assert_eq!(out.pending[0].expect, out.payload);
}

#[test]
#[should_panic(expected = "the lease authority cannot authorise as an owner")]
fn resolve_auth_refuses_to_name_the_authority() {
    let c = deploy();
    let _ = c.w_resolve_auth(vec![], auth_blob_for("login", AUTHORITY));
}

#[test]
#[should_panic(expected = "named account is not an owner")]
fn resolve_auth_refuses_an_account_that_is_not_an_owner() {
    let c = deploy();
    let _ = c.w_resolve_auth(vec![], auth_blob_for("login", "stranger.testnet"));
}

#[test]
#[should_panic(expected = "named account is not an owner")]
fn resolve_auth_panics_once_the_account_is_parked() {
    let mut c = deploy();
    arm(&mut c);
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(None, RotationCause::Reclaim);
    let _ = c.w_resolve_auth(vec![], auth_blob("login"));
}

#[test]
#[should_panic(expected = "authorization is not valid json")]
fn resolve_auth_panics_on_a_malformed_blob() {
    let c = deploy();
    let _ = c.w_resolve_auth(vec![], "not json".to_string());
}

#[test]
fn resolve_auth_serialises_to_the_nep641_wire_format() {
    let c = deploy();
    let out = near_sdk::serde_json::to_value(c.w_resolve_auth(vec![], auth_blob("login"))).unwrap();
    assert_eq!(out["payload"], "login");
    assert_eq!(out["pending"][0]["account_id"], OWNER);
    assert_eq!(out["pending"][0]["expect"], "login");
    assert!(
        out.get("status").is_none(),
        "the tagged status field is gone"
    );
}

#[test]
fn an_authority_freeze_lapses_on_its_own() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_freeze();
    assert_eq!(c.hos_lease().frozen, FreezeState::AuthorityFrozen);
    ctx(OWNER, 1, now_ns() + MAX_AUTHORITY_HOLD_NS);
    assert_eq!(
        c.hos_lease().frozen,
        FreezeState::Unfrozen,
        "a renter must regain the account without us acting"
    );
}

#[test]
fn agent_status_answers_the_same_as_the_calls_it_replaces() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(10), &["carol.testnet"]);
    let lease = c.hos_lease();
    let status = c.hos_agent_status(acc(BUYER));
    assert!(status.extension_enabled);
    assert_eq!(
        status.grant.map(|g| g.budget_yocto),
        c.hos_spend_grant(acc(BUYER)).map(|g| g.budget_yocto)
    );
    assert_eq!(status.state, lease.state);
    assert_eq!(status.frozen, lease.frozen);
    assert_eq!(status.lease_until_ns, lease.lease_until_ns);
    assert_eq!(status.impl_version, lease.impl_version);
}

#[test]
fn agent_status_reports_an_uninstalled_extension_without_a_grant() {
    let c = deploy();
    let status = c.hos_agent_status(acc(BUYER));
    assert!(!status.extension_enabled);
    assert!(status.grant.is_none());
}

#[test]
fn agent_status_sees_an_authority_freeze_lapse() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_freeze();
    assert_eq!(
        c.hos_agent_status(acc(BUYER)).frozen,
        FreezeState::AuthorityFrozen
    );
    ctx(OWNER, 1, now_ns() + MAX_AUTHORITY_HOLD_NS);
    assert_eq!(
        c.hos_agent_status(acc(BUYER)).frozen,
        FreezeState::Unfrozen,
        "an agent must not be told it is frozen after the hold lapses"
    );
}

#[test]
fn a_renter_freeze_does_not_lapse() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.hos_freeze();
    ctx(OWNER, 1, now_ns() + MAX_AUTHORITY_HOLD_NS * 4);
    assert_eq!(c.hos_lease().frozen, FreezeState::SelfFrozen);
}

#[test]
fn the_authority_can_refreeze_after_a_lapse() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_freeze();
    ctx(AUTHORITY, 1, now_ns() + MAX_AUTHORITY_HOLD_NS);
    c.hos_freeze();
    assert_eq!(c.hos_lease().frozen, FreezeState::AuthorityFrozen);
}

#[test]
#[should_panic(expected = "the owner has not authorised a transfer")]
fn the_authority_cannot_move_a_live_lease_the_owner_has_not_armed() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer);
}

#[test]
fn reclaim_after_expiry_needs_no_arming() {
    let mut c = init(now_ns(), now_ns() + 1);
    ctx(AUTHORITY, 1, now_ns() + 2);
    c.hos_transfer_ownership(None, RotationCause::Reclaim);
    assert_eq!(c.w_extensions().len(), 1);
}

#[test]
fn arming_is_consumed_by_a_transfer() {
    let mut c = deploy();
    arm(&mut c);
    assert!(c.hos_transfer_armed());
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer);
    assert!(!c.hos_transfer_armed());
}

#[test]
#[should_panic(expected = "only the owner")]
fn the_authority_cannot_arm_a_transfer_for_the_owner() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_arm_transfer(true);
}

#[test]
fn the_owner_can_disarm_before_a_sale_settles() {
    let mut c = deploy();
    arm(&mut c);
    ctx(OWNER, 1, now_ns());
    c.hos_arm_transfer(false);
    assert!(!c.hos_transfer_armed());
}

fn legacy_state(c: TenantWallet, armed: bool) -> LegacyTenantWallet {
    LegacyTenantWallet {
        wallet: c.wallet,
        authority: acc(AUTHORITY),
        owner: acc(OWNER),
        payout_account: acc(PAYOUT),
        lease_until_ns: c.lease_until_ns,
        state: OperatingState::Active,
        frozen: FreezeState::Unfrozen,
        authority_freeze_until_ns: 0,
        transfer_armed: armed,
    }
}

#[test]
fn migrate_names_the_owner_and_disarms_any_pending_transfer() {
    let c = deploy();
    let lease = c.lease_until_ns;
    let legacy = legacy_state(c, true);
    ctx(AUTHORITY, 0, now_ns());
    env::storage_write(STATE_KEY, &near_sdk::borsh::to_vec(&legacy).unwrap());
    let migrated = TenantWallet::hos_migrate(acc(OWNER));
    assert_eq!(migrated.owner, acc(OWNER));
    assert_eq!(migrated.lease_until_ns, lease);
    assert!(
        !migrated.transfer_armed,
        "a migration must not carry an armed transfer across"
    );
}

#[test]
#[should_panic(expected = "only the lease authority")]
fn migrate_is_refused_to_anyone_but_the_authority() {
    let c = deploy();
    let legacy = legacy_state(c, false);
    ctx(OWNER, 0, now_ns());
    env::storage_write(STATE_KEY, &near_sdk::borsh::to_vec(&legacy).unwrap());
    TenantWallet::hos_migrate(acc(OWNER));
}

#[test]
#[should_panic(expected = "only the owner")]
fn migrate_refuses_an_owner_outside_the_extension_set() {
    let c = deploy();
    let legacy = legacy_state(c, false);
    ctx(AUTHORITY, 0, now_ns());
    env::storage_write(STATE_KEY, &near_sdk::borsh::to_vec(&legacy).unwrap());
    TenantWallet::hos_migrate(acc("stranger.testnet"));
}

#[test]
#[should_panic(expected = "exceeds the granted cap")]
fn a_grant_cannot_be_split_across_promises_in_one_request() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(2), &["carol.testnet"]);
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([
        send("carol.testnet", NearToken::from_millinear(2)),
        send("carol.testnet", NearToken::from_millinear(2)),
    ]));
}

#[test]
#[should_panic(expected = "exceeds the granted cap")]
fn a_grant_is_consumed_and_does_not_reset_between_calls() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(3), &["carol.testnet"]);
    for _ in 0..2 {
        ctx(BUYER, 1, now_ns());
        c.w_execute_extension(
            Request::new().external([send("carol.testnet", NearToken::from_millinear(2))]),
        );
    }
}

#[test]
fn the_remaining_budget_is_visible_to_anyone() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(5), &["carol.testnet"]);
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(
        Request::new().external([send("carol.testnet", NearToken::from_millinear(2))]),
    );
    let grant = c.hos_spend_grant(acc(BUYER)).unwrap();
    assert_eq!(
        grant.spent_yocto.0,
        NearToken::from_millinear(2).as_yoctonear()
    );
    assert_eq!(
        grant.budget_yocto.0,
        NearToken::from_millinear(5).as_yoctonear()
    );
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

fn sweepable_from(balance: NearToken, c: &TenantWallet) -> u128 {
    balance.as_yoctonear().saturating_sub(c.reserve())
}

#[test]
fn a_sale_returns_the_balance_to_the_seller_not_the_buyer() {
    let mut c = deploy();
    arm(&mut c);
    let balance = NearToken::from_near(4);
    ctx_bal(AUTHORITY, 1, now_ns(), balance);
    let expected = sweepable_from(balance, &c);
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Sale);
    assert_eq!(transfers(), vec![(acc(PAYOUT), expected)]);
    assert_eq!(c.hos_payout_account(), acc(BUYER));
}

#[test]
fn the_authority_cannot_redirect_the_sweep() {
    let mut c = deploy();
    arm(&mut c);
    ctx_bal(AUTHORITY, 1, now_ns(), NearToken::from_near(4));
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Sale);
    let sent = transfers();
    assert!(
        sent.iter().all(|(to, _)| *to == acc(PAYOUT)),
        "the sweep destination is the wallet's own payout account, never the caller's choice"
    );
}

#[test]
fn a_recovery_leaves_the_balance_with_the_account() {
    let mut c = deploy();
    arm(&mut c);
    ctx_bal(AUTHORITY, 1, now_ns(), NearToken::from_near(4));
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Recovery);
    assert!(
        transfers().is_empty(),
        "a recovered account keeps its balance for the recovered owner"
    );
}

#[test]
fn a_reclaim_returns_the_balance_to_the_payout_account() {
    let mut c = init(now_ns(), now_ns() + 1);
    let balance = NearToken::from_near(6);
    ctx_bal(AUTHORITY, 1, now_ns() + 2, balance);
    let expected = sweepable_from(balance, &c);
    c.hos_transfer_ownership(None, RotationCause::Reclaim);
    assert_eq!(transfers(), vec![(acc(PAYOUT), expected)]);
}

#[test]
fn a_sweep_never_dips_into_the_reserve() {
    let mut c = deploy();
    arm(&mut c);
    ctx_bal(AUTHORITY, 1, now_ns(), NearToken::from_millinear(1));
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Sale);
    assert!(transfers().is_empty());
}

#[test]
fn a_rotation_onto_the_payout_account_moves_nothing() {
    let mut c = deploy();
    arm(&mut c);
    ctx_bal(AUTHORITY, 1, now_ns(), NearToken::from_near(4));
    c.hos_transfer_ownership(Some(acc(PAYOUT)), RotationCause::Sale);
    assert!(transfers().is_empty());
}

#[test]
fn the_sweep_leaves_the_account_able_to_pay_for_its_own_storage() {
    let mut c = deploy();
    arm(&mut c);
    let balance = NearToken::from_near(4);
    ctx_bal(AUTHORITY, 1, now_ns(), balance);
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Sale);
    let swept: u128 = transfers().iter().map(|(_, amount)| amount).sum();
    assert!(balance.as_yoctonear() - swept >= c.reserve());
}
