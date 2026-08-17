---
NEP: 0000
Title: Sharded Non-Fungible Token Items
Author: House of Stake
Status: Draft
DiscussionsTo: https://github.com/near/NEPs/discussions
Type: Standards Track
Category: Contract
Version: 1.0.0
Created: 2026-08-16
Updated: 2026-08-16
Requires: 171
---

## Summary

A standard for non-fungible tokens whose items are NEAR accounts rather than rows in a
collection's map, and for deciding whether such an account genuinely belongs to the collection
it names.

An item implements one view, `nft_item_info`, describing itself. A consumer pairs that answer
with the collection's `nft_token` and refuses unless the two agree. Everything else is NEP-171
as written.

## Motivation

NEP-171 gives a consumer exactly one way to ask about a token: `nft_token(token_id)` on the
contract, returning a `Token` or null. There is no normative mechanism for deciding whether a
`token_id` genuinely belongs to the contract being asked, and no statement about what happens
when a token is itself an account with its own state.

That is sufficient while a token is a row in a map. It stops being sufficient when the token is
a NEAR account that holds funds and acts on its own behalf. Two places then claim to know who
owns the thing, the account itself and the collection's index, and NEP-171 neither makes them
agree nor tells a consumer which to believe.

This standard closes that gap and makes the resulting trust explicit.

## Rationale and alternatives

### What TON does

TEP-62 splits an NFT into a collection contract and one contract per item. The item exposes
`get_nft_data() -> (init?, index, collection_address, owner_address, individual_content)`.

TEP-62 does not itself specify how to verify the relationship. TON's documentation names the
check and is explicit that the claim alone is worthless:

> Not every NFT that stores a collection address actually belongs to that collection. Verify
> that the collection returns the item's address for the item's index.

What makes that check work on TON is that a contract address is derived from a hash of its code
and initial data, so the collection can compute the address an item at index `i` must have.

### Why that does not port, and what replaces it

NEAR has no address derivation from code and data. It has something else that serves the same
purpose: a hierarchical account namespace with a protocol-enforced creation rule.
`alice.example.near` can only be created by `example.near`. Nobody else can produce that name,
at any price, by any transaction.

So the item's index is its own account id, read from `env::current_account_id()` rather than
stored, and the parent is a substring of that id. A consumer holding the account id already
knows which account must have created it, with no view call and nothing to trust.

This standard deliberately does not report the parent as a field. A consumer that has checked
`token_id` against the account it queried can derive the parent itself. Worse, a `parent_id`
field invites a consumer to check the parent and skip the identity check, which is the one
comparison that catches a contract lying about which token it is.

### Alternative considered: binding by code hash

Binding an item to its collection by asserting its deployed code hash is the apparent analogue
of TON's `hash(code, data)`. Whether it is correct depends on a choice this standard does not
make for the implementer:

- **Global contract by account id.** Code is replaced fleet-wide by the publisher. A code hash
  check is then meaningless between publishes and breaks every consumer during one. Membership
  rests on the namespace and on the governance of the publishing account.
- **Global contract by hash.** Code is immutable. A code hash check is then a durable structural
  binding, the closest NEAR equivalent to TEP-62's, at the cost of per-account migration to
  upgrade.

Implementations that value fleet upgradeability SHOULD choose the first and rely on the
namespace. Implementations that value immutability SHOULD choose the second and MAY additionally
assert the code hash. Neither is universally correct.

### Alternative considered: an on-chain verification callback

TEP-62 states its own limitation plainly: "There is no way to get current owner of NFT onchain
because TON is an asynchronous blockchain." NEAR is asynchronous in the same way, so an on-chain
caller reading an item through a cross-contract call inherits it exactly: by the time the
callback lands, ownership may have moved. This standard does not fix that and MUST NOT be
described as if it does.

On-chain consumers SHOULD invert the flow instead of verifying: require the collection to be the
actor, so that authority and action occur in the same receipt. A settlement contract calls
`nft_transfer` on the collection rather than verifying an item and then acting on it.

What NEAR permits and TON does not is an off-chain caller pinning both reads to one final block
height, so the pair cannot be read either side of a transfer. That is the sense in which the gap
closes, and it closes only for off-chain consumers.

## Specification

### Item Interface

```rust
pub struct NftItemInfo {
    pub spec: String,
    pub init: bool,
    pub status: ItemStatus,
    pub collection_id: AccountId,
    pub token_id: String,
    pub owner_id: AccountId,
    pub rotation_seq: U64,
    pub rotation_epoch: u32,
}

pub enum ItemStatus {
    Parked,
    Suspended,
    Expired,
    Frozen,
    Active,
}

pub fn nft_item_info(&self) -> NftItemInfo;
```

Against TEP-62's five fields, nothing is added to its shape, one is dropped, and three are new:

| TEP-62 | here | why |
| --- | --- | --- |
| `init?` | `init` | Same meaning: the item is initialised and has an owner. |
| `index` | `token_id` | The account id, which is this design's index. |
| `collection_address` | `collection_id` | A claim, load-bearing only once the collection confirms it. |
| `owner_address` | `owner_id` | The item is the authority here. |
| `individual_content` | absent | NEP-177 metadata is served by the collection. |
| | `spec` | Which revision of this interface the answer conforms to. |
| | `status` | Whether the item would accept work, which `init` alone cannot express. |
| | `rotation_seq` | Distinguishes an index that is catching up from one that is wrong. |
| | `rotation_epoch` | Which generation of stored state that sequence counts within. |

`token_id` is a `String` rather than an `AccountId` because NEP-171 defines it as a string, and
conformance is worth more than encoding an incidental invariant.

#### Requirements

1. `token_id` MUST be derived from `env::current_account_id()` and MUST NOT be read from stored
   state. A consumer derives the parent from it.
2. `collection_id` MUST NOT confer any authority. Naming a collection MUST NOT let an item do
   anything it could not otherwise do, because anyone can name any collection.
3. An item MUST refuse to name itself as its own collection at initialization.
4. `owner_id` MUST come from the state that actually governs the account, not a cached copy.
5. `spec` MUST be the version string of this standard the implementation conforms to. Consumers
   MUST ignore fields they do not recognise.
6. `status` MUST be `Active` if and only if the item would accept a request from an authorised
   caller at the current block. Every other value MUST name the first condition that would
   refuse it.
7. `rotation_seq` MUST increase on every rotation, including one that parks the item and
   therefore leaves `owner_id` unchanged. It counts rotations the collection records, not
   changes of owner, because a consumer must notice a park it would otherwise read as a
   still-valid name.
8. `rotation_epoch` MUST change whenever a migration restarts `rotation_seq`, and MUST NOT
   change otherwise. A sequence is meaningful only against another sequence from the same
   epoch. Migrations that introduce or replace the sequence may restart it, so an
   implementation that hid the restart would leave consumers unable to tell a fresh
   migration from a collection inventing rotations. Reporting the epoch is what allows the
   restart to be safe rather than silent.
9. An implementation MUST NOT report `Active` while any gate it enforces would refuse. Deriving
   `status` from the same predicate the contract already enforces is the only way to guarantee
   this; recomputing it separately will drift.

Requirement 4 is what makes the item worth asking. Requirement 6 is what stops a consumer
reconstructing liveness out of fields whose composition rule belongs to the implementation:
an item may be authentic, owned, and still unable to act.

### Collection Interface

A collection MUST implement NEP-171 as written. This standard adds no required collection method.

TEP-62 requires `get_nft_address_by_index()` because on TON an index and an address are
different things. Here the index *is* the address, so the round trip degenerates into
`nft_token(token_id)`, which NEP-171 already requires. A reader coming from TEP-62 should be
told this explicitly, because they will look for an address-by-index method and not find one.

A collection MAY additionally expose its own `rotation_seq` per token. When it does, a consumer
can type a disagreement by direction rather than retrying; when it does not, a consumer MUST
treat a disagreement as possibly transient.

### Verification

Verification is a ladder, not a single check. Each tier costs more and proves more, and an
implementation MUST state which tier it requires before which action.

| tier | cost | proves | sufficient for |
| --- | --- | --- | --- |
| T0 namespace | none, offline | only `<parent>` could have created this id | display, routing |
| T1 index | one call | the collection asserts membership and owner | enumeration, listing |
| T2 pair | two calls, one final block | the account itself agrees | transfer of value, authorisation |
| T3 provenance | cached, see below | `<parent>` could only have created legitimate ids | raising T0's meaning |

Consumers MUST perform T2 before any action that moves value or grants authority. Consumers
MUST NOT treat T1 alone as proof of ownership: enumeration is the collection's word, so a
compromised collection can list names an account does not own.

#### T2, the pair

```
item  = nft_item_info() on the account
token = nft_token(item.token_id) on the collection

accept only if:
  item.collection_id == the collection the consumer pinned from its own configuration
  item.token_id      == the account it was read from
  item.init
  token              is not null
  token.token_id     == item.token_id
  token.owner_id     == item.owner_id
```

Both reads MUST be pinned to the same block height, and that height MUST be final. Reading at
optimistic finality reintroduces the race the pinning exists to remove.

The collection MUST be pinned by the consumer from its own configuration. Taking it from
`item.collection_id` makes the first clause vacuous and defeats the check, because an attacker
can then supply both halves.

Each clause rules out a distinct forgery and none is redundant:

- **The item alone is not enough.** Any contract at any account can return an `nft_item_info`
  naming any collection. The collection half refuses it.
- **The collection alone is not enough.** `nft_token` is a map lookup. It says the collection
  believes an account is a member; it says nothing about what that account currently does.
- **The identity clause is what makes the rest mean anything.** A contract may return any
  `token_id`, so comparing it against the account actually queried is what stops an item
  answering for a token it is not.
- **The owner comparison catches divergence.** The item and the index update in separate
  receipts.

#### Divergence is expected, and its direction is the signal

The item and the index are updated in separate receipts, so they disagree briefly on every
rotation. A consumer MUST NOT treat every disagreement as evidence of compromise.

When both halves report a `rotation_seq` **and the same `rotation_epoch`**:

| observation | meaning | consumer action |
| --- | --- | --- |
| `item > index` | the index has not caught up | retry, bounded |
| `item < index` | the index claims a rotation the account never made | refuse, do not retry |
| equal, owners differ | genuine corruption | refuse, do not retry |

A consumer MUST compare sequences only within one epoch. Across an epoch change the numbers
describe different generations of stored state, and the lower one is the newer: a migration
restarts the count, so treating `item < index` as an invented rotation would report every
migrated account as corrupt.

When either half reports no sequence, or the two report different epochs, a consumer cannot
tell direction and MUST treat an owner disagreement as retryable, refusing until it converges
or a bound elapses.

#### T3, and the horizon on caching it

Where the parent account holds no key, the only remaining creator of `<label>.<parent>` is the
parent's own contract, and the chain is public:

1. the account id ends with `.<parent>`, which only `<parent>` could have produced;
2. `view_access_key_list(<parent>)` is empty, so its minting authority is code, not a person;
3. `<parent>` names the collection it mints for, and mints only when that collection asks.

Steps 2 and 3 are properties of the collection, not of any item, so a consumer MAY check them
once and cache the result, after which membership of any name is a suffix comparison: free,
offline, and available to a client that has never seen the account before. This is strictly
cheaper than TEP-62 can express, because a TON address does not distinguish who may produce
siblings.

**This cache has a horizon and implementations MUST state it.** Steps 2 and 3 are statements
about mutable state. Where the parent's contract is upgradeable, an upgrade can change what it
will mint, and a consumer holding a cached proof has no way to observe that. A collection
SHOULD therefore expose a monotonic configuration epoch, and a consumer that caches T3 SHOULD
re-validate when it changes. A consumer that cannot observe an epoch MUST bound the lifetime of
its cached proof.

TON's per-item round trip is stateless and therefore never stale. That is the cost this
optimisation trades away, and a standard that claims the benefit without naming the cost is
incomplete.

### Trust model

The collection is trusted to say who is a member. The item is trusted to say who owns it.
Neither is trusted for the other's answer, and a consumer that accepts a disagreement has no
defence against whichever side is wrong.

A collection able to rotate an item's ownership through its authority can move a name. The pair
check detects the resulting divergence but does not prevent the rotation. That is the residual
trust and it is not removed by removing keys, because the authority is exercised by code.

**Authenticity is not suitability.** A verified pair says this account is genuinely the name it
claims and is owned by whom it says. `status` reports whether it can currently act. Consumers
trading the asset MUST read both: a name whose lease has expired, or which is frozen, is still
authentic.

## Reference Implementation

Item, `contracts/hos-wallet/src/lib.rs`, `nft_item_info` and `item_status`. Unit tests in
`src/tests.rs`, `mod sharded_item`: an item describes itself; the reported owner follows a
transfer rather than a stale index; `token_id` is read from the account rather than stored; a
parked item reports `init: false`; an item cannot name itself as its own collection; a frozen
item is authentic but not `Active`; an expired lease reports `Expired`; `status` is `Active`
exactly when the account would accept work; every rotation advances the sequence.

End to end against real wasm, `integration/tests/sharded_item.rs`: a minted name passes and a
wallet deployed off-collection claiming the registry is refused; a forgery created under the
real parent is still refused by the collection clause; a keyless parent makes the forgery
impossible to create at all, and the same test asserts the empty key list and the parent naming
its collection, so the whole cached chain is proven in one place; a pair whose halves disagree
is refused.

Consumer, `packages/contract-client/src/item.ts`: the ladder, the pinned final-block pair, the
direction-typed divergence, and a refusal reason per clause, each tested including an attacker
supplying both halves.

## Changelog

### 1.0.0 - Initial version

## Copyright

Copyright and related rights waived via [CC0](https://creativecommons.org/publicdomain/zero/1.0/).
