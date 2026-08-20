# House of Stake Top Level Accounts

Contracts for renting names under a Top Level Account. A rented name is a real NEAR account created
under the TLA, running a shared wallet implementation, leased to an owner for a fixed term.

A leased account holds no access key. Its wallet is initialised with signature checking switched off,
so nothing signs on the account's behalf. Control is a set of account ids held in the account's own
contract. The owner acts by sending an ordinary transaction from an account they already control, which
can be a seed phrase, a passkey, a ledger, a multisig or a DAO. House of Stake holds a second entry in
that set so a lease can be reclaimed and a sale settled without the owner taking part.

## The contracts

| Contract | Role |
| --- | --- |
| `hos-wallet` | The tenant wallet. Published once as a global contract and referenced by every leased account. Holds the extension set, the lease window, the payout account and the freeze state. |
| `wallet-impl-deployer` | Publishes `hos-wallet`. A code hash is approved first, then anyone may publish code matching it. |
| `registrar` | Mints leased accounts. Creates the sub-account, funds it, attaches the wallet implementation and initialises it in one batch. |
| `tla-registry` | The lease ledger and the NEP-171 collection. Rental, renewal, pricing, reclaim, business sub-account rules, and the only contract holding money. |
| `hos-extension` | Acts on leased accounts on behalf of the registry. Pushes lease updates, forces transfers, sweeps a reclaimed account. |
| `mpc-recovery` | Recovery for both account kinds. For an ordinary NEAR account a watcher quorum authorises an MPC-signed `AddKey` after a timelock. For a leased name the same policy gates a rotation performed by the registry. |
| `hos-common` | Types and helpers shared by more than one contract. |

`dev-contracts` holds stub fungible token, staking pool, dapp and MPC signer contracts. They exist so
the integration tests can exercise real cross-contract paths, and are never deployed.

## Controlling a leased account

`create_sub_account` is callable only by the registry. It builds the account in one batch: create,
transfer the funding, point the account at the wallet implementation as a global contract, then call
`hos_init`.

`hos_init` in turn only accepts a call from the account's direct parent, so an account can only be
brought up by the registrar that created it. Initialisation puts exactly two entries in the extension
set, the owner's account and the authority the registrar supplies, and rejects a configuration where
those two are the same or where either is the account itself.

To move funds or call a contract, the owner sends a transaction from their own account calling
`w_execute_extension` on the leased account. The caller check runs in two stages. Membership in the
extension set admits the caller to the function. Anything external, meaning a transfer or a call to
another contract, additionally requires the caller not to be the authority. So the authority passes the
first check and is refused by the second, and cannot spend from a leased account. The test is "not the
authority" rather than "is the renter", so any co-owner the renter adds inherits the same rights. An
owner can add and remove co-owners through the same path.

An owner can also delegate spending without handing over a key. `hos_grant_spend` gives one extension a
scope: the accounts it may pay, a ceiling in NEAR, a budget per fungible token, and a list of the exact
items it may move. A grant covers plain transfers, `ft_transfer` and `nft_transfer`, and nothing else,
because those are the only calls whose arguments state what leaves the account. The amount in an
`ft_transfer` is read on chain and charged against that token's budget. Without that the ceiling would
be decorative, since the deposit attached to a token call is one yocto while the sum being moved sits in
the arguments.

Non-fungible assets are fenced rather than budgeted. There is no quantity to meter, so the grant names
the token ids that may leave, and a count would be a false ceiling: five transfers of a collectible and
five transfers of an account holding real balances are the same number. A grant can never name this
account's own collection, which is what stops an agent moving or listing a name its owner holds.
Re-granting raises the ceilings without clearing what has been spent, so topping up an agent cannot
un-spend the meter. Revoking is the way to reset it.

While a lease runs, the owner alone decides who holds the account. `assert_extension_editor` lets the
authority reach the extension set only once the lease has expired, which is the reclaim window. The
authority can never remove itself, expired or not.

`w_resolve_auth` implements NEP-641. It never resolves an authorization itself. It returns `Pending`
listing every extension except the authority, which points a caller at the accounts that can actually
authorise. If no such extension remains, it returns `Invalid`.

The authority is fixed at initialisation and has no setter. That is what makes reclaim possible.

## The lease

`hos_set_lease` is authority only. The new expiry must be greater than or equal to the current one, so
a lease can be extended but never shortened, and the call refuses to set the `Parked` state.

`hos_transfer_ownership` clears the extension set down to the authority, adds the new owner if one is
given, and moves the payout account to match. Co-owners the previous owner added do not survive. There
is no key to rotate and no token to move, so a sale, a reclaim or a recovery all end the previous
owner's control the same way.

The wallet takes `hos_transfer_ownership` from the authority alone, and what constrains it sits above.
`hos-extension` accepts a rotation only from the registry, and every registry path that reaches one
carries its own gate: a plain transfer or a sale requires the caller to be the current owner, a reclaim
requires the lease to have ended, and a recovery requires an attested request, the account's own
timelock and the watcher quorum. There is no path by which an admin or the council moves a live name.

A rotation caused by recovery is also refused while the owner holds their own freeze. Reclaim after
expiry is not, since a freeze that outlived the lease would otherwise strand the name.

The payout account is read before it is repointed, so the balance leaves with the party giving the name
up rather than the one receiving it. `RotationCause` decides both halves of that: `Deposit`, used when a
name goes into a marketplace venue, is the one cause that does not repoint the payout, and `Recovery`
and `Revert` are the two that do not sweep.

Freezing runs in both directions. Any extension may call `hos_freeze`. A freeze is recorded as
`SelfFrozen` when the caller is not the authority and `AuthorityFrozen` when it is. `hos_unfreeze`
enforces the matching side, so the authority cannot lift an owner's freeze and an owner cannot lift the
authority's. An authority freeze lapses on its own after seven days, so a lost or misused authority key
cannot strand an account, and a further seven days must pass before the authority can freeze again, so
the seven day limit cannot be renewed into a standing hold. An owner's own freeze does not lapse.

Sweeping is gated on the authority and an expired lease. `assert_sweepable` checks those two and does
not read the freeze state, so a frozen account can still be swept once its lease has ended.

## Renting and pricing

Rent is held in USD micro units and converted to yoctoNEAR at call time using a NEAR/USD rate the
registry stores. `pricing.rs` bounds that conversion: a quote carries a slippage allowance in basis
points, and a rate update is rejected unless it falls inside a floor and ceiling derived from the
previous rate. Rent itself is derived from the label length and the premium category of the name.

The lifecycle states a sub-account can hold are `Registered`, `Active`, `Grace`, `Reclaimable` and
`Suspended`.

## Names as tokens

`tla-registry` is the collection. A name is a NEP-171 token whose id is the full account id, with
NEP-177 metadata, NEP-181 enumeration and NEP-297 events. Minting a name emits `nft_mint`, every
ownership rotation emits `nft_transfer`, and reclaim emits `nft_burn`.

NEP-178 approvals are deliberately absent. `nft_transfer` rejects a set `approval_id` rather than
ignoring it, because settlement always passes `None` and an approval nobody uses is attack surface
nobody audits. NEP-199 payouts are absent for the same reason: nothing is taken on resale.

Because a token here is an account holding its own funds rather than a row in the collection's map, two
places claim to know who owns it. Each leased account answers `nft_item_info` describing itself, and a
consumer pairs that with the collection's `nft_token` and refuses unless the two agree. That pattern is
written up as a NEP draft in `docs/sharded-nft-items.md`, and the registrar's `config_epoch` exists to
serve it: it moves on every upgrade and configuration change so a consumer caching a membership proof
knows when to re-check.

## Marketplace

There is no marketplace in these contracts. There are no listings, no offers and no commission. Resale
settles on NEAR Intents, and the registry's job is to keep a seller's claim attached to the seller
while their name is in someone else's custody.

A name is deposited with `nft_transfer_call` to a venue the council has allowlisted. That rotation
carries `RotationCause::Deposit`, which leaves the payout account pointing at the seller, so proceeds
and any later sweep still reach them rather than the venue. All three entry points derive the cause
from the receiver, so a venue reached through a plain `nft_transfer` gets the same treatment.

Gates sit on the way in and never on the way out. Depositing a name, or transferring one to another
user, is refused while the account holds a balance of an allowlisted fungible token. Withdrawing a name
from a venue is not gated at all, by anything: not the token balance, not a pause, not the TLA's status,
not the lease lifecycle. A refused entrance is harmless because the asset stays where it was, while a
refused exit is a trap, and a name that merely accrued an unsolicited token would otherwise be stuck in
custody permanently.

That token check is hygiene rather than a control, and it is worth stating plainly. It covers only the
council's allowlist, and it only runs at the entrance, so an account can be emptied, deposited, and then
funded while it sits in the venue. NEAR is different and is genuinely enforced, because the rotation
sweeps it to the outgoing payout account.

The registry keeps a per-account refund balance. `claim_refund` pays the caller whatever is recorded
against their own account id, clearing the entry before the transfer and restoring it if the transfer
fails. `withdraw` reserves every pending refund before releasing anything, so a treasury withdrawal
cannot spend money a user is owed.

Business TLAs add a sub-account cap and a scheduled retraction. Scheduling and cancelling both sit with
the licensee, so the party that can start the notice period is the party that can stop it.

## Reclaim

When a lease ends, the registry sweeps the account before releasing the name. `reclaim_sweep_near` and
`reclaim_sweep_ft` move balances to the payout account, and `reclaim_finalize` completes the release.
Each step checks balances in a callback rather than assuming them. A frozen account can still be swept
once the lease has ended, since neither freeze survives expiry.

## Recovery

`mpc-recovery` covers both account kinds. An ordinary NEAR account has no wallet contract to call and so
cannot be recovered by rewriting an extension set; it is recovered by adding a key. A leased name is
recovered by rotating its owner through the registry. Both run through the same per-account policy.

A policy per account holds an MPC public key, an attestation key and a timelock. Both keys must be
ed25519 and the timelock is bounded at both ends.

Installation is delegated but not unilateral. The contract holds an `installer` alongside its owner, set
by the owner through `set_installer` and defaulting to the owner. Before the installer can create a
policy, the account itself must call `arm_policy_install` naming the attestation key and timelock it
consents to, and the install must match that arming exactly, which consumes it. An account can withdraw
its consent with `disarm_policy_install` at any time before the install. A leased name holds no key of
its own, so its owner arms it by driving the account through `w_execute_extension`.

Replacing an existing policy is owner only, because a policy carries the attestation key that authorises
starting a recovery, and an installer able to rewrite it could point an armed account at a key of its
own. Abort is open to the installer, the owner and the account under recovery: it can only deny a
recovery, never grant one, so the brake must not be slower than the automated path it stops.
`PolicyInstalled` carries both keys, so a rotation is visible on chain rather than silent.

A recovery request carries the new owner key, the current round, and a signature over a request message
made with the attestation key. The round increments on every request, so a replayed request is stale.

After the timelock has elapsed, watchers submit a verdict signed by a quorum. Their public keys and the
threshold are fixed when the contract is initialised. A rejection returns the account to idle. An
approval lets the installer or the owner call `finalize_recovery`, which asks the MPC signer for a
signature over an `AddKey` transaction for the account.

A leased name is recovered through the same policy rather than around it. `request_name_recovery` checks
the request against the attestation key pinned for that account and starts its timelock, and only then
does `recover_name` accept a watcher quorum and ask the registry to rotate the owner. The quorum is
signed over the round the request recorded, so a quorum gathered for one round cannot settle another,
and the registry call is followed by a callback that restores the request if the rotation fails rather
than consuming it.

This is not trustless. It trusts the watcher set by design.

## Publishing the wallet implementation

`wallet-impl-deployer` splits approval from publication. The council approves a code hash, which stamps
the approval time. Once 48 hours have passed, anyone may call `gd_deploy` with base64 encoded code
matching that hash, attaching the global storage cost, which is charged per byte and refunded
above the amount used. The encoding is load bearing rather than cosmetic: passing the code as a JSON
number array costs roughly three times the argument bytes and has to be parsed one number at a time,
which took the publish of a 347 KB implementation to 296 Tgas against a 300 Tgas ceiling. A successful deploy records the hash and clears the approval. A failed one
refunds the deposit and leaves the approval usable, so a retry does not need a second approval. Only
one deploy runs at a time.

`approved_at` and `approval_delay_ns` are views, so the window is readable on chain rather than
promised. `migrate` carries pre-delay state forward and drops any approval pending at the time, so it
has to be re-approved and serve the delay.

Two things would otherwise let the delay be skipped, and both are closed in the contract.

A full access key on the deployer account can publish with a `DeployGlobalContract` action and never
touch `gd_approve`, because tenant wallets follow `use_global_contract_by_account_id` and the whole
fleet moves the moment that account publishes. No contract can refuse a protocol action taken with its
own account's key, but it can drop the key: `gd_delete_key` is council only and removes one from the
deployer account.

`upgrade_self` would have been the other way round it, since the council could install a build with no
delay and publish immediately after. It now serves the same window as the code it governs, through
`approve_self_upgrade` and then a wait, and the approval is spent by the upgrade it authorises.

Together those make the window enforceable: once the keys are off, the only way to change the account
is a governed upgrade that waits out the delay itself.

There is deliberately no `locked` view. A contract cannot enumerate its own access keys, so a flag
would assert something it cannot check. `view_access_key_list` on the deployer account is the signal,
and an empty result is the proof.

Keys are still present on testnet so the fleet can be iterated without waiting out the window. Removing
them is a mainnet step, and until it happens the delay binds the contract path only.

## Trust boundaries

The contracts assume the following and do not re-check them.

Leased accounts reference the wallet implementation by account id rather than by hash, so republishing
changes the code under every leased account at once. That is deliberate, so a fault can be patched
without House of Stake being locked out, and it makes code approval the highest privilege action here.
The 48 hour delay between approval and publication bounds it: the code under an account cannot change
without a window in which the pending hash is already visible on chain.

`hos-extension` is the authority on every leased account, so replacing its code rewrites transfer,
lease and sweep behaviour everywhere at once. It carries the same rule as the wallet implementation: a
code hash is approved first and cannot be published until 48 hours have passed.

`push_lease`, `force_transfer`, `sweep_near` and `sweep_ft` on `hos-extension` each call
`assert_registry`, so only `tla-registry` can reach them. The registrar passes its configured
`hos_extension` as the `authority` in every `hos_init`, which is how one account ends up holding the
authority seat on every leased account. `mpc-recovery` holds a transfer authority fixed when it is
initialised.

Both `tla-registry` and `hos-extension` separate a council from an operations admin set. Council holds
what changes the business or hands out power: adding and removing admins, the fee model, releasing
revenue, registering a TLA, granting the payment and recovery authorities, approving an extension
upgrade, the price oracle account and the initial rate, and a business sub-account cap. Pricing sits
with council because the oracle account and the rate together set what every name costs. Operations
keeps the day to day surface, including both pause switches so an incident does not wait on a multisig.
Admin rights are not self-granting: an operations key cannot add its own co-admins, so a key compromise
is a rotation rather than a permanent loss. Council is a constructor parameter on both contracts, since
on mainnet it is a DAO rather than a fixed account.

Every privileged method demands exactly one yoctoNEAR. The protocol only lets a full access key attach
a deposit, so a restricted function-call key can never reach one of these, and a key handed to a script
cannot escalate into governance. That holds uniformly: registering a TLA, adding or removing an admin,
a payment authority or a recovery authority, changing the fee model, releasing revenue, setting a
business cap, adding or removing a venue, approving and running an upgrade, sealing, and on
`mpc-recovery` rotating the watcher set, delegating the installer and pointing at the registry.

The accounts these contracts pay are fixed when they are initialised and have no setters. `treasury`
receives revenue released by council and anything skimmed from the extension, and changing it takes a
contract upgrade rather than a transaction. The same is true of `council` itself, the registry and
extension each contract points at, and the recovery owner. That is deliberate, and it means the
deployment parameters have to be right before the first `new` rather than corrected afterwards.

Pausing is scoped, and both switches close entrances rather than exits. `pause` halts the registry.
`pause_marketplace` halts only handing a name to a venue; taking one back out is a plain `nft_transfer`
and stays open, so a pause cannot strand a name in custody. Renting, renewing, transfers, reclaim and
recovery keep running while the marketplace is paused. `is_paused` and `is_marketplace_paused` are
separate views.

A pause also cannot hold a user's property. Sweeps deliberately skip the pause check, and a holder can
always renew, which is asserted by a test module named for the rule rather than for the function.

Withdrawal destinations are fixed at deployment. `withdraw` on the registry and `skim` on the extension
take an amount but not a recipient, so an admin can release funds and cannot redirect them.

## Events and JSON types

Every `u64` and `u128` crossing the JSON boundary uses the `near_sdk::json_types` wrappers, so it
arrives as a string. A raw JSON number loses precision in a consumer parsing with a double, and
nanosecond timestamps are already past that point. This holds for arguments, views and events alike.

Events carry typed values. Nothing is `Debug` formatted as wire data, enums are not stringly typed, an
absent value is not an empty string, and no field changes meaning depending on which branch emitted it.

## Layout

```
contracts/         the six deployed contracts
crates/            shared library code
dev-contracts/     stubs used only by the integration tests
integration/       near-workspaces tests, a separate cargo workspace
```

`integration` is a separate cargo workspace and loads built wasm rather than depending on the contract
crates. Deployment tooling and the account map live outside this repository.

## Build and test

The toolchain is pinned in `rust-toolchain.toml` and each contract pins its reproducible build image in
its own `Cargo.toml`. Building needs `cargo-near`.

```
cargo near build non-reproducible-wasm --locked --no-abi
```

Artifacts land in `target/near/<name>/<name>.wasm`, which is where the integration tests look for them.
`near-sdk` rejects a bare `cargo build` for a contract crate, so use `cargo check --target
wasm32-unknown-unknown` to type-check without producing artifacts.

The reproducible build is a different thing and the difference matters at deploy time:

```
cargo near build reproducible-wasm --no-abi
```

It builds the source at the current git commit inside the pinned Docker image, not the working tree.
Uncommitted work is absent from the artifact, and it writes to the same path as the non-reproducible
build, so a stale artifact overwrites a fresh one with no warning. Commit first, then build, then
deploy, then tag. Deploying an artifact built from an older commit onto an account whose state a newer
build already migrated leaves the account unreadable: the deployed struct is missing the fields the
stored state carries, and borsh refuses the trailing bytes, so every method panics with `Cannot
deserialize the contract state`. Recovery is to deploy the matching code; nothing about the state is
lost.

```
cargo test
cd integration && cargo test
```

The integration suite runs against a nearcore sandbox and needs every contract built first. It touches
no live network.

## Status

Pre-audit. Not deployed to mainnet.

`w_resolve_auth` implements NEP-641, which is not final and has no reference implementation yet. Those
shapes will move.

`docs/sharded-nft-items.md` is a draft, not a submitted NEP. It has no number yet, because a NEP takes
the number of the pull request that proposes it.

Three limitations are deliberate rather than outstanding, and are described where they apply: the
fungible token check on transfers is hygiene and not a laundering control, `mpc-recovery` can sign
`AddKey` but only for accounts that already hold an MPC-derived key, and the deployer on testnet runs
with its publish delay set to zero so the fleet can be iterated. The last of those is a mainnet
blocker and `config` reports it.
