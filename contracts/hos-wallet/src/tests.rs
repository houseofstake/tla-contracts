use super::*;
use defuse_wallet::actions::FunctionCall;
use near_sdk::test_utils::VMContextBuilder;
use near_sdk::testing_env;

const WALLET: &str = "alice.tla.testnet";
const PARENT: &str = "tla.testnet";
const AUTHORITY: &str = "hos-extension.testnet";
const PAYOUT: &str = "payout.testnet";
const OWNER: &str = "renter.testnet";
const REGISTRY: &str = "registry.testnet";
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
        collection_id: acc(REGISTRY),
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
    install_and_grant_tokens(c, cap, receivers, &[]);
}

fn install_and_grant_tokens(
    c: &mut TenantWallet,
    cap: NearToken,
    receivers: &[&str],
    tokens: &[(&str, u128)],
) {
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    grant_tokens(c, cap, receivers, tokens);
}

fn grant_tokens(c: &mut TenantWallet, cap: NearToken, receivers: &[&str], tokens: &[(&str, u128)]) {
    grant_full(c, cap, receivers, tokens, &[]);
}

fn grant_items(c: &mut TenantWallet, receivers: &[&str], items: &[(&str, &[&str])]) {
    grant_full(c, NearToken::ZERO, receivers, &[], items);
}

fn install_and_grant_items(c: &mut TenantWallet, receivers: &[&str], items: &[(&str, &[&str])]) {
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    grant_items(c, receivers, items);
}

fn grant_full(
    c: &mut TenantWallet,
    cap: NearToken,
    receivers: &[&str],
    tokens: &[(&str, u128)],
    items: &[(&str, &[&str])],
) {
    ctx(OWNER, 1, now_ns());
    c.hos_grant_spend(
        acc(BUYER),
        receivers.iter().map(|r| acc(r)).collect(),
        U128(cap.as_yoctonear()),
        tokens
            .iter()
            .map(|(token, budget)| TokenAllowance {
                token: acc(token),
                budget: U128(*budget),
            })
            .collect(),
        items
            .iter()
            .map(|(collection, token_ids)| ItemAllowance {
                collection: acc(collection),
                token_ids: token_ids.iter().map(|id| (*id).to_string()).collect(),
            })
            .collect(),
        U64(now_ns() + HOUR_NS),
    );
}

fn nft_transfer(collection: &str, to: &str, token_id: &str) -> NearPromise {
    NearPromise::new(acc(collection)).function_call(
        FunctionCall::name("nft_transfer")
            .args_json(near_sdk::serde_json::json!({
                "receiver_id": to,
                "token_id": token_id,
            }))
            .attach_deposit(NearToken::from_yoctonear(1)),
    )
}

fn ft_transfer(token: &str, to: &str, amount: u128) -> NearPromise {
    NearPromise::new(acc(token)).function_call(
        FunctionCall::name("ft_transfer")
            .args_json(near_sdk::serde_json::json!({
                "receiver_id": to,
                "amount": U128(amount),
            }))
            .attach_deposit(NearToken::from_yoctonear(1)),
    )
}

fn legacy_grants(grants: BTreeMap<AccountId, SpendGrant>) -> BTreeMap<AccountId, LegacySpendGrant> {
    grants
        .into_iter()
        .map(|(extension, grant)| {
            (
                extension,
                LegacySpendGrant {
                    receivers: grant.receivers,
                    budget_yocto: grant.budget_yocto,
                    spent_yocto: grant.spent_yocto,
                    expires_at: grant.expires_at,
                },
            )
        })
        .collect()
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
fn a_revert_returns_the_name_to_where_it_came_from_without_a_fresh_arming() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
    assert!(c.w_is_extension_enabled(acc(BUYER)));
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(OWNER)), RotationCause::Revert, None);
    assert!(c.w_is_extension_enabled(acc(OWNER)));
    assert!(!c.w_is_extension_enabled(acc(BUYER)));
}

#[test]
#[should_panic(expected = "only return the name to where it came from")]
fn a_revert_cannot_send_the_name_to_a_third_party() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc("attacker.testnet")), RotationCause::Revert, None);
}

#[test]
#[should_panic(expected = "no rotation to revert")]
fn a_revert_cannot_run_without_a_rotation_to_undo() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(OWNER)), RotationCause::Revert, None);
}

#[test]
#[should_panic(expected = "no rotation to revert")]
fn a_revert_is_one_shot() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(OWNER)), RotationCause::Revert, None);
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(OWNER)), RotationCause::Revert, None);
}

#[test]
#[should_panic(expected = "revert window for this rotation has closed")]
fn a_revert_expires_with_its_window() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
    ctx(AUTHORITY, 1, now_ns() + HOUR_NS);
    c.hos_transfer_ownership(Some(acc(OWNER)), RotationCause::Revert, None);
}

#[test]
#[should_panic(expected = "must be asked for by an account that holds it")]
fn a_revert_does_not_let_the_authority_move_the_name_onward() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(OWNER)), RotationCause::Revert, None);
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(BUYER)));
}

#[test]
#[should_panic(expected = "only return the name to where it came from")]
fn a_revert_reaches_only_the_previous_hop_not_further_back() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(
        Some(acc("carol.testnet")),
        RotationCause::Transfer,
        Some(acc(BUYER)),
    );
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(OWNER)), RotationCause::Revert, None);
}

#[test]
#[should_panic(expected = "receiver must not be this account")]
fn a_rotation_cannot_target_the_wallet_account() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(WALLET)), RotationCause::Transfer, Some(acc(OWNER)));
}

#[test]
#[should_panic(expected = "only the lease authority")]
fn the_owner_cannot_rotate_the_wallet_directly() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
}

#[test]
#[should_panic(expected = "only the lease authority")]
fn a_granted_extension_cannot_rotate_the_wallet() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(10), &["carol.testnet"]);
    ctx(BUYER, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Revert, None);
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
        vec![],
        vec![],
        U64(now_ns() + YEAR_NS),
    );
}

#[test]
#[should_panic(expected = "token is not in the spend grant")]
fn a_grantee_cannot_reach_a_token_the_grant_never_named() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::from_millinear(1),
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([ft_transfer(
        "usdt.testnet",
        "carol.testnet",
        1,
    )]));
}

#[test]
fn a_grantee_can_move_a_granted_token_within_its_budget() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([ft_transfer(
        "usdc.testnet",
        "carol.testnet",
        40,
    )]));
    let grant = c.hos_spend_grant(acc(BUYER)).unwrap();
    assert_eq!(
        grant.tokens.get(&acc("usdc.testnet")).unwrap().spent.0,
        40,
        "a token spend must be metered in that token's own units"
    );
    assert_eq!(
        grant.spent_yocto.0, 0,
        "the yocto a token demands as proof of signature is not spend"
    );
}

#[test]
#[should_panic(expected = "spend exceeds the granted cap for this token")]
fn a_token_budget_bounds_the_amount_inside_the_arguments() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([ft_transfer(
        "usdc.testnet",
        "carol.testnet",
        101,
    )]));
}

#[test]
#[should_panic(expected = "spend exceeds the granted cap for this token")]
fn token_spending_accumulates_across_calls() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    for _ in 0..2 {
        ctx(BUYER, 1, now_ns());
        c.w_execute_extension(Request::new().external([ft_transfer(
            "usdc.testnet",
            "carol.testnet",
            60,
        )]));
    }
}

#[test]
#[should_panic(expected = "receiver is not in the spend grant")]
fn a_token_call_cannot_pay_an_account_outside_the_grant() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([ft_transfer(
        "usdc.testnet",
        "attacker.testnet",
        1,
    )]));
}

#[test]
#[should_panic(expected = "ft_transfer and nft_transfer only")]
fn a_grantee_cannot_call_ft_transfer_call() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    ctx(BUYER, 1, now_ns());
    let call = NearPromise::new(acc("usdc.testnet")).function_call(
        FunctionCall::name("ft_transfer_call")
            .args_json(near_sdk::serde_json::json!({
                "receiver_id": "carol.testnet",
                "amount": U128(1),
                "msg": "",
            }))
            .attach_deposit(NearToken::from_yoctonear(1)),
    );
    c.w_execute_extension(Request::new().external([call]));
}

#[test]
#[should_panic(expected = "arguments are not readable")]
fn a_token_call_carrying_an_unreadable_argument_is_refused() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    ctx(BUYER, 1, now_ns());
    let call = NearPromise::new(acc("usdc.testnet")).function_call(
        FunctionCall::name("ft_transfer")
            .args_json(near_sdk::serde_json::json!({
                "receiver_id": "carol.testnet",
                "amount": U128(1),
                "surprise": "unread by this contract",
            }))
            .attach_deposit(NearToken::from_yoctonear(1)),
    );
    c.w_execute_extension(Request::new().external([call]));
}

#[test]
#[should_panic(expected = "attaches exactly one yocto")]
fn a_token_call_cannot_carry_a_deposit_of_its_own() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    ctx(BUYER, 1, now_ns());
    let call = NearPromise::new(acc("usdc.testnet")).function_call(
        FunctionCall::name("ft_transfer")
            .args_json(near_sdk::serde_json::json!({
                "receiver_id": "carol.testnet",
                "amount": U128(1),
            }))
            .attach_deposit(NearToken::from_near(1)),
    );
    c.w_execute_extension(Request::new().external([call]));
}

#[test]
#[should_panic(expected = "may carry no other action")]
fn a_token_call_cannot_smuggle_a_transfer_alongside_it() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    ctx(BUYER, 1, now_ns());
    let call = ft_transfer("usdc.testnet", "carol.testnet", 1).transfer(NearToken::from_near(1));
    c.w_execute_extension(Request::new().external([call]));
}

#[test]
#[should_panic(expected = "never deploying code")]
fn a_grantee_cannot_deploy_code_through_a_state_init() {
    use defuse_wallet::StateInitV1;

    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_near(1), &["carol.testnet"]);
    ctx(BUYER, 1, now_ns());
    let deployment = NearPromise::new(acc("carol.testnet")).deterministic_state_init(
        StateInitV1::code(acc("global.testnet")),
        NearToken::from_millinear(1),
    );
    c.w_execute_extension(Request::new().external([deployment]));
}

#[test]
fn topping_up_a_grant_does_not_un_spend_the_meter() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([ft_transfer(
        "usdc.testnet",
        "carol.testnet",
        60,
    )]));
    grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 200)],
    );
    let grant = c.hos_spend_grant(acc(BUYER)).unwrap();
    assert_eq!(
        grant.tokens.get(&acc("usdc.testnet")).unwrap().spent.0,
        60,
        "raising a ceiling must not hand the agent back what it already spent"
    );
}

#[test]
fn revoking_a_grant_resets_the_meter() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([ft_transfer(
        "usdc.testnet",
        "carol.testnet",
        60,
    )]));
    ctx(OWNER, 1, now_ns());
    c.hos_revoke_spend(acc(BUYER));
    grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 100)],
    );
    let grant = c.hos_spend_grant(acc(BUYER)).unwrap();
    assert_eq!(
        grant.tokens.get(&acc("usdc.testnet")).unwrap().spent.0,
        0,
        "revoking is the documented way to clear the meter"
    );
}

#[test]
fn a_grantee_can_move_an_item_the_grant_fenced() {
    let mut c = deploy();
    install_and_grant_items(
        &mut c,
        &["carol.testnet"],
        &[("art.testnet", &["1041", "1055"])],
    );
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([nft_transfer(
        "art.testnet",
        "carol.testnet",
        "1041",
    )]));
}

#[test]
fn an_item_fence_is_identity_not_a_count() {
    let mut c = deploy();
    install_and_grant_items(&mut c, &["carol.testnet"], &[("art.testnet", &["1041"])]);
    for _ in 0..3 {
        ctx(BUYER, 1, now_ns());
        c.w_execute_extension(Request::new().external([nft_transfer(
            "art.testnet",
            "carol.testnet",
            "1041",
        )]));
    }
}

#[test]
#[should_panic(expected = "item is not in the spend grant")]
fn a_grantee_cannot_move_an_item_outside_the_fence() {
    let mut c = deploy();
    install_and_grant_items(&mut c, &["carol.testnet"], &[("art.testnet", &["1041"])]);
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([nft_transfer(
        "art.testnet",
        "carol.testnet",
        "9000",
    )]));
}

#[test]
#[should_panic(expected = "collection is not in the spend grant")]
fn a_grantee_cannot_reach_a_collection_the_grant_never_named() {
    let mut c = deploy();
    install_and_grant_items(&mut c, &["carol.testnet"], &[("art.testnet", &["1041"])]);
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([nft_transfer(
        "other.testnet",
        "carol.testnet",
        "1041",
    )]));
}

#[test]
#[should_panic(expected = "cannot move this account's own names")]
fn a_grant_cannot_fence_the_registry_at_all() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    grant_items(
        &mut c,
        &["carol.testnet"],
        &[(REGISTRY, &["bob.tla.testnet"])],
    );
}

#[test]
#[should_panic(expected = "cannot move this account's own names")]
fn a_grant_cannot_budget_the_registry_as_a_token_either() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[(REGISTRY, 1_000)],
    );
}

#[test]
#[should_panic(expected = "cannot move this account's own names")]
fn the_registry_is_refused_at_spend_time_on_the_token_path_too() {
    let mut c = deploy();
    install_and_grant_tokens(
        &mut c,
        NearToken::ZERO,
        &["carol.testnet"],
        &[("usdc.testnet", 1_000)],
    );
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([ft_transfer(REGISTRY, "carol.testnet", 1)]));
}

#[test]
#[should_panic(expected = "cannot move this account's own names")]
fn a_grantee_cannot_move_a_name_even_if_the_registry_slipped_into_a_grant() {
    let mut c = deploy();
    install_and_grant_items(&mut c, &["carol.testnet"], &[("art.testnet", &["1041"])]);
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([nft_transfer(
        REGISTRY,
        "carol.testnet",
        "bob.tla.testnet",
    )]));
}

#[test]
#[should_panic(expected = "cannot spend an approval")]
fn a_granted_item_transfer_cannot_spend_an_approval() {
    let mut c = deploy();
    install_and_grant_items(&mut c, &["carol.testnet"], &[("art.testnet", &["1041"])]);
    ctx(BUYER, 1, now_ns());
    let call = NearPromise::new(acc("art.testnet")).function_call(
        FunctionCall::name("nft_transfer")
            .args_json(near_sdk::serde_json::json!({
                "receiver_id": "carol.testnet",
                "token_id": "1041",
                "approval_id": 7u64,
            }))
            .attach_deposit(NearToken::from_yoctonear(1)),
    );
    c.w_execute_extension(Request::new().external([call]));
}

#[test]
#[should_panic(expected = "receiver is not in the spend grant")]
fn an_item_transfer_cannot_pay_an_account_outside_the_grant() {
    let mut c = deploy();
    install_and_grant_items(&mut c, &["carol.testnet"], &[("art.testnet", &["1041"])]);
    ctx(BUYER, 1, now_ns());
    c.w_execute_extension(Request::new().external([nft_transfer(
        "art.testnet",
        "attacker.testnet",
        "1041",
    )]));
}

#[test]
#[should_panic(expected = "ft_transfer and nft_transfer only")]
fn a_grantee_cannot_hand_out_an_approval() {
    let mut c = deploy();
    install_and_grant_items(&mut c, &["carol.testnet"], &[("art.testnet", &["1041"])]);
    ctx(BUYER, 1, now_ns());
    let call = NearPromise::new(acc("art.testnet")).function_call(
        FunctionCall::name("nft_approve")
            .args_json(near_sdk::serde_json::json!({
                "token_id": "1041",
                "account_id": "attacker.testnet",
            }))
            .attach_deposit(NearToken::from_yoctonear(1)),
    );
    c.w_execute_extension(Request::new().external([call]));
}

#[test]
#[should_panic(expected = "ft_transfer and nft_transfer only")]
fn a_grantee_cannot_call_nft_transfer_call() {
    let mut c = deploy();
    install_and_grant_items(&mut c, &["carol.testnet"], &[("art.testnet", &["1041"])]);
    ctx(BUYER, 1, now_ns());
    let call = NearPromise::new(acc("art.testnet")).function_call(
        FunctionCall::name("nft_transfer_call")
            .args_json(near_sdk::serde_json::json!({
                "receiver_id": "carol.testnet",
                "token_id": "1041",
                "msg": "",
            }))
            .attach_deposit(NearToken::from_yoctonear(1)),
    );
    c.w_execute_extension(Request::new().external([call]));
}

#[test]
#[should_panic(expected = "an item grant needs at least one token id")]
fn an_empty_item_list_grants_nothing_and_says_so() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    grant_items(&mut c, &["carol.testnet"], &[("art.testnet", &[])]);
}

#[test]
fn the_owner_keeps_the_item_path_a_grantee_is_fenced_on() {
    let mut c = deploy();
    install_and_grant_items(&mut c, &["carol.testnet"], &[("art.testnet", &["1041"])]);
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(Request::new().external([nft_transfer(
        REGISTRY,
        "carol.testnet",
        "bob.tla.testnet",
    )]));
}

#[test]
fn agent_status_reports_the_reserve_floor() {
    let c = deploy();
    let status = c.hos_agent_status(acc(BUYER));
    assert_eq!(status.reserve_yocto.0, c.reserve());
    assert!(status.reserve_yocto.0 > 0);
}

#[test]
fn the_owner_keeps_the_function_call_path_a_grantee_loses() {
    let mut c = deploy();
    install_and_grant(&mut c, NearToken::from_millinear(1), &["usdc.testnet"]);
    ctx(OWNER, 1, now_ns());
    let call = NearPromise::new(acc("usdc.testnet")).function_call(
        FunctionCall::name("ft_transfer").attach_deposit(NearToken::from_yoctonear(1)),
    );
    c.w_execute_extension(Request::new().external([call]));
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
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(
        Some(acc("newowner.testnet")),
        RotationCause::Sale,
        Some(acc(OWNER)),
    );
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
fn a_co_owner_the_owner_installed_may_ask_for_a_transfer() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(
        Some(acc("carol.testnet")),
        RotationCause::Transfer,
        Some(acc(BUYER)),
    );
    assert!(c.w_is_extension_enabled(acc("carol.testnet")));
    assert!(!c.w_is_extension_enabled(acc(BUYER)));
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
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
    assert!(c.w_is_extension_enabled(acc(BUYER)));
    assert!(!c.w_is_extension_enabled(acc(OWNER)));
    assert_eq!(c.hos_payout_account(), acc(BUYER));
}

#[test]
fn the_authority_sets_the_payout_account() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_set_payout_account(acc(BUYER), acc(OWNER));
    assert_eq!(c.hos_payout_account(), acc(BUYER));
}

#[test]
#[should_panic(expected = "only the lease authority")]
fn the_renter_cannot_set_their_own_payout_account() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.hos_set_payout_account(acc(BUYER), acc(OWNER));
}

#[test]
#[should_panic(expected = "payout account must not be this account")]
fn the_payout_account_cannot_be_the_leased_account_itself() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_set_payout_account(env::current_account_id(), acc(OWNER));
}

#[test]
#[should_panic(expected = "changed hands before this payout change landed")]
fn a_payout_change_authorised_by_the_previous_owner_is_refused() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
    ctx(AUTHORITY, 1, now_ns());
    c.hos_set_payout_account(acc("carol.testnet"), acc(OWNER));
}

#[test]
fn a_deposit_moves_ownership_but_leaves_payout_with_the_depositor() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(
        Some(acc("venue.testnet")),
        RotationCause::Deposit,
        Some(acc(OWNER)),
    );
    assert!(c.w_is_extension_enabled(acc("venue.testnet")));
    assert_eq!(
        c.hos_payout_account(),
        acc(PAYOUT),
        "a venue never earns a claim on the balance, so the next sweep must not reach it"
    );
}

#[test]
fn leaving_a_venue_repoints_payout_at_the_new_holder() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(
        Some(acc("venue.testnet")),
        RotationCause::Deposit,
        Some(acc(OWNER)),
    );
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(
        Some(acc(BUYER)),
        RotationCause::Transfer,
        Some(acc("venue.testnet")),
    );
    assert_eq!(c.hos_payout_account(), acc(BUYER));
}

#[test]
fn a_transfer_evicts_co_owners() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner("carol.testnet")]));
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
    assert!(!c.w_is_extension_enabled(acc("carol.testnet")));
    assert!(!c.w_is_extension_enabled(acc(OWNER)));
}

#[test]
fn parking_leaves_only_the_authority_and_keeps_the_payout_account() {
    let mut c = init(now_ns(), now_ns() + 1);
    ctx(AUTHORITY, 1, now_ns() + 2);
    c.hos_transfer_ownership(None, RotationCause::Reclaim, None);
    assert_eq!(c.w_extensions().len(), 1);
    assert!(c.w_is_extension_enabled(acc(AUTHORITY)));
    assert_eq!(c.hos_payout_account(), acc(PAYOUT));
}

#[test]
#[should_panic(expected = "only the lease authority")]
fn the_renter_cannot_transfer_ownership() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
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
        collection_id: acc(REGISTRY),
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
        collection_id: acc(REGISTRY),
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
#[should_panic(expected = "a spend grantee cannot authorise as an owner")]
fn resolve_auth_refuses_an_extension_that_only_holds_a_spend_grant() {
    let mut c = deploy();
    ctx(OWNER, 1, now_ns());
    c.w_execute_extension(request([add_co_owner(BUYER)]));
    ctx(OWNER, 1, now_ns());
    c.hos_grant_spend(
        acc(BUYER),
        vec![acc(PAYOUT)],
        U128(1),
        vec![],
        vec![],
        U64(now_ns() + 1_000_000_000),
    );
    let _ = c.w_resolve_auth(vec![], auth_blob_for("login", BUYER));
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
    let mut c = init(now_ns(), now_ns() + 1);
    ctx(AUTHORITY, 1, now_ns() + 2);
    c.hos_transfer_ownership(None, RotationCause::Reclaim, None);
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
#[should_panic(expected = "must be asked for by an account that holds it")]
fn the_authority_cannot_move_a_live_lease_on_its_own() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, None);
}

#[test]
#[should_panic(expected = "unauthorized")]
fn the_authority_cannot_name_itself_as_the_holder() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(
        Some(acc(BUYER)),
        RotationCause::Transfer,
        Some(acc(AUTHORITY)),
    );
}

#[test]
#[should_panic(expected = "must be asked for by an account that holds it")]
fn a_stranger_cannot_be_named_as_the_holder() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(
        Some(acc(BUYER)),
        RotationCause::Transfer,
        Some(acc("attacker.testnet")),
    );
}

#[test]
fn reclaim_after_expiry_needs_no_holder() {
    let mut c = init(now_ns(), now_ns() + 1);
    ctx(AUTHORITY, 1, now_ns() + 2);
    c.hos_transfer_ownership(None, RotationCause::Reclaim, None);
    assert_eq!(c.w_extensions().len(), 1);
}

#[test]
#[should_panic(expected = "sweep requires an expired lease")]
fn the_authority_cannot_reclaim_a_live_lease() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(None, RotationCause::Reclaim, None);
}

#[test]
fn recovery_works_without_the_lost_owner_doing_anything() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Recovery, None);
    assert!(c.w_is_extension_enabled(acc(BUYER)));
    assert!(!c.w_is_extension_enabled(acc(OWNER)));
}

#[test]
fn an_owner_who_leaves_cannot_ask_for_a_further_transfer() {
    let mut c = deploy();
    ctx(AUTHORITY, 1, now_ns());
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
    ctx(AUTHORITY, 1, now_ns());
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        c.hos_transfer_ownership(
            Some(acc("carol.testnet")),
            RotationCause::Transfer,
            Some(acc(OWNER)),
        );
    }));
    assert!(outcome.is_err(), "the previous owner is no longer a holder");
}

fn legacy_state(c: TenantWallet) -> LegacyTenantWallet {
    LegacyTenantWallet {
        wallet: c.wallet,
        authority: acc(AUTHORITY),
        owner: acc(OWNER),
        collection_id: acc(REGISTRY),
        payout_account: acc(PAYOUT),
        lease_until_ns: c.lease_until_ns,
        state: OperatingState::Active,
        frozen: FreezeState::Unfrozen,
        authority_freeze_until_ns: 0,
        spend_grants: legacy_grants(c.spend_grants),
        revert_to: None,
        revert_until_ns: 0,
        rotation_seq: c.rotation_seq,
    }
}

#[test]
fn migrate_carries_the_owner_across() {
    let c = deploy();
    let lease = c.lease_until_ns;
    let legacy = legacy_state(c);
    ctx(AUTHORITY, 0, now_ns());
    env::storage_write(STATE_KEY, &near_sdk::borsh::to_vec(&legacy).unwrap());
    let migrated = TenantWallet::hos_migrate(acc(REGISTRY));
    assert_eq!(migrated.owner, acc(OWNER));
    assert_eq!(migrated.lease_until_ns, lease);
    assert!(migrated.wallet.extensions.contains(&acc(OWNER)));
}

#[test]
fn migrate_cannot_be_used_to_hand_the_wallet_to_another_account() {
    let c = deploy();
    let mut legacy = legacy_state(c);
    legacy.wallet.extensions.insert(acc("co-owner.testnet"));
    ctx(AUTHORITY, 0, now_ns());
    env::storage_write(STATE_KEY, &near_sdk::borsh::to_vec(&legacy).unwrap());
    let migrated = TenantWallet::hos_migrate(acc(REGISTRY));
    assert_eq!(
        migrated.owner,
        acc(OWNER),
        "the authority must not be able to name a new owner through a migration"
    );
}

#[test]
#[should_panic(expected = "only the lease authority")]
fn migrate_is_refused_to_anyone_but_the_authority() {
    let c = deploy();
    let legacy = legacy_state(c);
    ctx(OWNER, 0, now_ns());
    env::storage_write(STATE_KEY, &near_sdk::borsh::to_vec(&legacy).unwrap());
    TenantWallet::hos_migrate(acc(REGISTRY));
}

#[test]
#[should_panic(expected = "only the owner")]
fn migrate_refuses_an_owner_outside_the_extension_set() {
    let c = deploy();
    let mut legacy = legacy_state(c);
    legacy.owner = acc("stranger.testnet");
    ctx(AUTHORITY, 0, now_ns());
    env::storage_write(STATE_KEY, &near_sdk::borsh::to_vec(&legacy).unwrap());
    TenantWallet::hos_migrate(acc(REGISTRY));
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
    let balance = NearToken::from_near(4);
    ctx_bal(AUTHORITY, 1, now_ns(), balance);
    let expected = sweepable_from(balance, &c);
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Sale, Some(acc(OWNER)));
    assert_eq!(transfers(), vec![(acc(PAYOUT), expected)]);
    assert_eq!(c.hos_payout_account(), acc(BUYER));
}

#[test]
fn the_authority_cannot_redirect_the_sweep() {
    let mut c = deploy();
    ctx_bal(AUTHORITY, 1, now_ns(), NearToken::from_near(4));
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Sale, Some(acc(OWNER)));
    let sent = transfers();
    assert!(
        sent.iter().all(|(to, _)| *to == acc(PAYOUT)),
        "the sweep destination is the wallet's own payout account, never the caller's choice"
    );
}

#[test]
fn a_recovery_leaves_the_balance_with_the_account() {
    let mut c = deploy();
    ctx_bal(AUTHORITY, 1, now_ns(), NearToken::from_near(4));
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Recovery, None);
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
    c.hos_transfer_ownership(None, RotationCause::Reclaim, None);
    assert_eq!(transfers(), vec![(acc(PAYOUT), expected)]);
}

#[test]
fn a_sweep_never_dips_into_the_reserve() {
    let mut c = deploy();
    ctx_bal(AUTHORITY, 1, now_ns(), NearToken::from_millinear(1));
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Sale, Some(acc(OWNER)));
    assert!(transfers().is_empty());
}

#[test]
fn a_rotation_onto_the_payout_account_moves_nothing() {
    let mut c = deploy();
    ctx_bal(AUTHORITY, 1, now_ns(), NearToken::from_near(4));
    c.hos_transfer_ownership(Some(acc(PAYOUT)), RotationCause::Sale, Some(acc(OWNER)));
    assert!(transfers().is_empty());
}

#[test]
fn the_sweep_leaves_the_account_able_to_pay_for_its_own_storage() {
    let mut c = deploy();
    let balance = NearToken::from_near(4);
    ctx_bal(AUTHORITY, 1, now_ns(), balance);
    c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Sale, Some(acc(OWNER)));
    let swept: u128 = transfers().iter().map(|(_, amount)| amount).sum();
    assert!(balance.as_yoctonear() - swept >= c.reserve());
}

mod deployed_shape {
    use crate::{LegacyTenantWallet, TenantWallet};
    use near_sdk::base64::Engine;
    use near_sdk::borsh::BorshDeserialize;

    const DEPLOYED_STATE_B64: &str = "AAAAAAAQDgAAAAAAAAAAAAAAAAAAAAAAAAIAAAATAAAAZXh0Lmhvc2RlbW8udGVzdG5ldBMAAABmdW5kci1ob3MtNS50ZXN0bmV0EwAAAGV4dC5ob3NkZW1vLnRlc3RuZXQTAAAAZnVuZHItaG9zLTUudGVzdG5ldBgAAAByZWdpc3RyeS5ob3NkZW1vLnRlc3RuZXQTAAAAZnVuZHItaG9zLTUudGVzdG5ldHM+cEIcUjkZAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==";

    fn raw() -> Vec<u8> {
        near_sdk::base64::engine::general_purpose::STANDARD
            .decode(DEPLOYED_STATE_B64)
            .expect("fixture is valid base64")
    }

    #[test]
    fn the_legacy_struct_still_matches_a_deployed_wallet() {
        let old = LegacyTenantWallet::try_from_slice(&raw())
            .expect("deployed wallet no longer decodes as LegacyTenantWallet");
        assert_eq!(old.authority.as_str(), "ext.hosdemo.testnet");
        assert_eq!(old.owner.as_str(), "fundr-hos-5.testnet");
        assert_eq!(
            old.collection_id.as_str(),
            "registry.hosdemo.testnet",
            "collection_id sits where a stale legacy struct expects payout_account, \
             so reading the wrong value here is the first sign of drift"
        );
        assert_eq!(old.payout_account.as_str(), "fundr-hos-5.testnet");
        assert_eq!(old.revert_until_ns, 0);
        assert_eq!(old.rotation_seq, 0);
        assert!(old.spend_grants.is_empty());
        assert!(old.wallet.extensions.contains(&old.owner));
        assert!(old.wallet.extensions.contains(&old.authority));
    }

    #[test]
    fn a_deployed_wallet_loads_under_this_code_without_migrating() {
        let live = TenantWallet::try_from_slice(&raw()).expect(
            "publishing global code every leased account cannot read is a fleet-wide outage \
             that lasts until a migration lands on each one. If this fails, the layout changed \
             and the release needs a migration sweep planned before the publish, not after",
        );
        assert_eq!(live.owner.as_str(), "fundr-hos-5.testnet");
        assert_eq!(live.authority.as_str(), "ext.hosdemo.testnet");
        assert_eq!(live.collection_id.as_str(), "registry.hosdemo.testnet");
    }
}

mod adapter_wire_format {
    use defuse_wallet::Request;

    const ADAPTER_REQUEST: &str = r#"{
      "request": {
        "external": [
          {
            "receiver_id": "counter.testnet",
            "actions": [
              {
                "action": "function_call",
                "payload": {
                  "function_name": "increment",
                  "args": "eyJieSI6MX0=",
                  "deposit": "0",
                  "gas": "30000000000000"
                }
              },
              {
                "action": "transfer",
                "payload": { "amount": "1000000000000000000000000" }
              }
            ]
          }
        ]
      }
    }"#;

    #[derive(near_sdk::serde::Deserialize)]
    #[serde(crate = "near_sdk::serde")]
    struct ExecuteArgs {
        request: Request,
    }

    #[test]
    fn the_adapter_json_deserialises_as_a_request() {
        let args: ExecuteArgs = near_sdk::serde_json::from_str(ADAPTER_REQUEST)
            .expect("adapter JSON is not a valid w_execute_extension argument");
        assert_eq!(args.request.external.len(), 1);
        let promise = &args.request.external[0];
        assert_eq!(promise.receiver_id.as_str(), "counter.testnet");
        assert_eq!(promise.actions.len(), 2);
        assert!(promise.refund_to.is_none());
        assert_eq!(
            promise.total_deposit(),
            near_sdk::NearToken::from_near(1),
            "the transfer amount must survive the wire"
        );
    }
}

mod sharded_item {
    use super::*;

    #[test]
    fn an_item_describes_itself_without_the_collection() {
        let c = deploy();
        let info = c.nft_item_info();
        assert_eq!(info.collection_id, acc(REGISTRY));
        assert_eq!(info.token_id, WALLET);
        assert_eq!(info.owner_id, acc(OWNER));
        assert!(info.init);
    }

    #[test]
    fn the_reported_owner_follows_a_transfer_not_the_stale_index() {
        let mut c = deploy();
        ctx(AUTHORITY, 1, now_ns());
        c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
        assert_eq!(
            c.nft_item_info().owner_id,
            acc(BUYER),
            "the item is the authority on ownership, so a collection index that still \
             says otherwise is the thing that is wrong"
        );
    }

    #[test]
    fn the_token_id_is_the_account_itself_and_not_a_stored_value() {
        let c = deploy();
        assert_eq!(
            c.nft_item_info().token_id,
            env::current_account_id().to_string(),
            "a caller derives the parent from this, so it must never be something \
             the item could have chosen"
        );
    }

    #[test]
    fn a_parked_item_reports_itself_uninitialised() {
        let mut c = init(now_ns(), now_ns() + 1);
        ctx(AUTHORITY, 1, now_ns() + 2);
        c.hos_transfer_ownership(None, RotationCause::Reclaim, None);
        assert!(!c.nft_item_info().init);
    }

    #[test]
    #[should_panic(expected = "collection account must not be this account")]
    fn an_item_cannot_claim_to_be_its_own_collection() {
        ctx(PARENT, 0, now_ns());
        TenantWallet::hos_init(WalletInit {
            owner_account: acc(OWNER),
            authority: acc(AUTHORITY),
            collection_id: acc(WALLET),
            payout_account: acc(PAYOUT),
            lease_until_ns: U64(now_ns() + YEAR_NS),
            timeout_secs: 3600,
        });
    }

    #[test]
    fn an_item_names_the_revision_of_the_interface_it_answers_for() {
        assert_eq!(deploy().nft_item_info().spec, SHARDED_ITEM_SPEC);
    }

    #[test]
    fn a_live_name_reports_itself_active() {
        assert_eq!(deploy().nft_item_info().status, ItemStatus::Active);
    }

    #[test]
    fn a_frozen_item_is_still_authentic_but_is_no_longer_active() {
        let mut c = deploy();
        ctx(OWNER, 1, now_ns());
        c.hos_freeze();
        let info = c.nft_item_info();
        assert!(
            info.init,
            "freezing does not un-own the account, so init stays true and a consumer \
             reading only init would think the name can act"
        );
        assert_eq!(info.status, ItemStatus::Frozen);
    }

    #[test]
    fn an_expired_lease_reports_expired_rather_than_active() {
        let c = init(now_ns(), now_ns() + 1);
        ctx(OWNER, 0, now_ns() + 2);
        assert_eq!(c.nft_item_info().status, ItemStatus::Expired);
    }

    #[test]
    fn a_parked_item_reports_parked_before_any_other_reason() {
        let mut c = init(now_ns(), now_ns() + 1);
        ctx(AUTHORITY, 1, now_ns() + 2);
        c.hos_transfer_ownership(None, RotationCause::Reclaim, None);
        assert_eq!(c.nft_item_info().status, ItemStatus::Parked);
    }

    #[test]
    fn status_active_means_exactly_that_the_account_would_accept_work() {
        let mut c = deploy();
        assert_eq!(c.nft_item_info().status, ItemStatus::Active);
        ctx(OWNER, 1, now_ns());
        c.hos_freeze();
        assert_ne!(c.nft_item_info().status, ItemStatus::Active);
        ctx(OWNER, 1, now_ns());
        c.hos_unfreeze();
        assert_eq!(
            c.nft_item_info().status,
            ItemStatus::Active,
            "status is the same predicate assert_renter_active uses, so the two can \
             never disagree about whether a request would be refused"
        );
    }

    #[test]
    fn every_rotation_advances_the_sequence_so_a_stale_index_is_detectable() {
        let mut c = deploy();
        let before = c.nft_item_info().rotation_seq.0;
        ctx(AUTHORITY, 1, now_ns());
        c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
        let after = c.nft_item_info().rotation_seq.0;
        assert_eq!(
            after,
            before + 1,
            "an index carrying a lower sequence is behind and will catch up; one \
             carrying a higher sequence claims a rotation the account never made"
        );
    }

    #[test]
    fn parking_a_name_advances_the_sequence_even_though_the_owner_is_unchanged() {
        let mut c = init(now_ns(), now_ns() + 1);
        let before = c.nft_item_info().rotation_seq.0;
        ctx(AUTHORITY, 1, now_ns() + 2);
        c.hos_transfer_ownership(None, RotationCause::Reclaim, None);
        let info = c.nft_item_info();
        assert_eq!(
            info.owner_id,
            acc(OWNER),
            "a park does not hand the name to anyone"
        );
        assert_eq!(
            info.rotation_seq.0,
            before + 1,
            "the sequence counts rotations rather than changes of owner, because a \
             park is a rotation the collection records and a consumer must notice"
        );
        assert_eq!(info.status, ItemStatus::Parked);
    }

    #[test]
    fn the_sequence_is_only_comparable_within_one_rotation_epoch() {
        let c = deploy();
        let info = c.nft_item_info();
        assert_eq!(
            info.rotation_epoch, ROTATION_EPOCH,
            "an item states which generation its sequence belongs to, so a consumer \
             refuses to compare two sequences that never shared a counter"
        );
    }

    #[test]
    fn a_migration_that_carries_the_sequence_leaves_the_epoch_alone() {
        let mut c = deploy();
        ctx(AUTHORITY, 1, now_ns());
        c.hos_transfer_ownership(Some(acc(BUYER)), RotationCause::Transfer, Some(acc(OWNER)));
        let before = c.nft_item_info();
        let carried = before.rotation_seq.0;
        assert!(carried > 0, "the fixture needs a sequence worth carrying");

        let legacy = LegacyTenantWallet {
            wallet: c.wallet,
            authority: acc(AUTHORITY),
            owner: acc(BUYER),
            collection_id: acc(REGISTRY),
            payout_account: acc(BUYER),
            lease_until_ns: c.lease_until_ns,
            state: OperatingState::Active,
            frozen: FreezeState::Unfrozen,
            authority_freeze_until_ns: 0,
            spend_grants: legacy_grants(c.spend_grants),
            revert_to: c.revert_to,
            revert_until_ns: c.revert_until_ns,
            rotation_seq: carried,
        };
        ctx(AUTHORITY, 0, now_ns());
        env::storage_write(STATE_KEY, &near_sdk::borsh::to_vec(&legacy).unwrap());
        let migrated = TenantWallet::hos_migrate(acc(REGISTRY));
        let after = migrated.nft_item_info();

        assert_eq!(
            after.rotation_seq.0, carried,
            "this migration does not restart the counter, so it must carry it"
        );
        assert_eq!(
            after.rotation_epoch, before.rotation_epoch,
            "the epoch moves only with a reset. Moving it here would tell every holder \
             their recorded sequence is incomparable when it is still the same counter"
        );
    }
}
