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
| `tla-registry` | The lease ledger. Rental, renewal, pricing, marketplace, reclaim and business sub-account rules. |
| `hos-extension` | Acts on leased accounts on behalf of the registry. Pushes lease updates, forces transfers, sweeps a reclaimed account. |
| `mpc-recovery` | Recovery for ordinary NEAR accounts, which have no wallet contract to call. A watcher quorum authorises an MPC-signed `AddKey` after a timelock. |
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
`w_execute_extension` on the leased account. The wallet checks the caller is an enabled extension and
runs the request. An owner can add and remove co-owners through the same path.

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

Freezing runs in both directions. Any extension may call `hos_freeze`. A freeze is recorded as
`SelfFrozen` when the caller is not the authority and `AuthorityFrozen` when it is. `hos_unfreeze`
enforces the matching side, so the authority cannot lift an owner's freeze and an owner cannot lift the
authority's.

Sweeping is gated on the authority and an expired lease. `assert_sweepable` checks those two and does
not read the freeze state, so a frozen account can still be swept once its lease has ended.

## Renting and pricing

Rent is held in USD micro units and converted to yoctoNEAR at call time using a NEAR/USD rate the
registry stores. `pricing.rs` bounds that conversion: a quote carries a slippage allowance in basis
points, and a rate update is rejected unless it falls inside a floor and ceiling derived from the
previous rate. Rent itself is derived from the label length and the premium category of the name.

The lifecycle states a sub-account can hold are `Registered`, `Active`, `Grace`, `Reclaimable` and
`Suspended`.

## Marketplace

A name can be listed at an ask, unlisted, sold outright, or sold by accepting an offer made against it.
Transfers and sales complete in callbacks rather than in the entrypoint. `split_resale` divides a sale
price into a commission, taken in basis points, and the remainder.

The registry keeps a per-account refund balance. `claim_refund` pays the caller whatever is recorded
against their own account id, and fails if there is nothing pending or the contract cannot cover it.

Business TLAs add a sub-account cap and a scheduled retraction. A retraction is set with a deadline the
registry stores and can be cancelled before it lands.

## Reclaim

When a lease ends, the registry sweeps the account before releasing the name. `reclaim_sweep_near` and
`reclaim_sweep_ft` move balances to the payout account, and `reclaim_finalize` completes the release.
Each step checks balances in a callback rather than assuming them. A frozen account can still be swept
once the lease has ended, since neither freeze survives expiry.

## Recovery

`mpc-recovery` covers ordinary NEAR accounts, which have no wallet contract to call and therefore
cannot be recovered by rewriting an extension set.

The owner installs a policy per account holding an MPC public key, an attestation key and a timelock.
Both keys must be ed25519 and the timelock is bounded at both ends.

A recovery request carries the new owner key, the current round, and a signature over a request message
made with the attestation key. The round increments on every request, so a replayed request is stale.

After the timelock has elapsed, watchers submit a verdict signed by a quorum. Their public keys and the
threshold are fixed when the contract is initialised. A rejection returns the account to idle. An
approval lets the owner call `finalize_recovery`, which asks the MPC signer for a signature over an
`AddKey` transaction for the account.

This is not trustless. It trusts the watcher set by design.

## Publishing the wallet implementation

`wallet-impl-deployer` splits approval from publication. The council or the patch authority approves a
code hash. After that anyone may call `gd_deploy` with code matching that hash, attaching the global
storage cost, which is charged per byte and refunded above the amount used. A successful deploy records
the hash and clears the approval. A failed one refunds the deposit and leaves the approval usable, so a
retry does not need a second approval. Only one deploy runs at a time.

`upgrade_self` is council only.

## Trust boundaries

The contracts assume the following and do not re-check them.

Leased accounts reference the wallet implementation by account id rather than by hash, so republishing
changes the code under every leased account at once. That is deliberate, so a fault can be patched
without House of Stake being locked out, and it makes code approval the highest privilege action here.

The patch authority approves a code hash with the same power as the council.

`push_lease`, `force_transfer`, `sweep_near` and `sweep_ft` on `hos-extension` each call
`assert_registry`, so only `tla-registry` can reach them. The registrar passes its configured
`hos_extension` as the `authority` in every `hos_init`, which is how one account ends up holding the
authority seat on every leased account. `hos-extension` keeps its own admin set and pause switch,
separate from the registry. `mpc-recovery` holds a transfer authority fixed when it is initialised.

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
deployment.testnet.json
```

`integration` is a separate cargo workspace and loads built wasm rather than depending on the contract
crates.

`deployment.testnet.json` records the testnet account map, the published wallet code hash and the
registrar's minting parameters. It describes what is on chain, which may lag the working tree.

## Build and test

The toolchain is pinned in `rust-toolchain.toml` and each contract pins its reproducible build image in
its own `Cargo.toml`. Building needs `cargo-near`.

```
cargo near build non-reproducible-wasm --locked --no-abi
```

Artifacts land in `target/near/<name>/<name>.wasm`, which is where the integration tests look for them.
`near-sdk` rejects a bare `cargo build` for a contract crate, so use `cargo check --target
wasm32-unknown-unknown` to type-check without producing artifacts.

```
cargo test
cd integration && cargo test
```

The integration suite runs against a nearcore sandbox and needs every contract built first. Two tests
in `integration/tests/testnet_deploy.rs` are ignored by default because they deploy to live testnet.

## Status

Pre-audit. Not deployed to mainnet.

`w_resolve_auth` implements NEP-641, which is not final and has no reference implementation yet. Those
shapes will move.
