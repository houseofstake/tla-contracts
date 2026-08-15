# Sharded NFT items on NEAR

Status: draft, implemented in this repository and covered by tests.

## Problem

NEP-171 gives a consumer exactly one way to ask about a token: `nft_token(token_id)` on the
contract, which returns a `Token` or null. There is no normative mechanism in NEP-171 for deciding
whether a `token_id` genuinely belongs to the contract you are asking, and no statement about what
happens when a token is itself an account with its own state.

That is fine while a token is a row in a contract's map. It stops being fine when the token is a
NEAR account that holds funds and acts on its own behalf, which is what a leased name is here. Then
there are two places that claim to know who owns the thing: the account itself, and the registry's
index. Nothing in NEP-171 makes them agree, and nothing tells a wallet which to believe.

## What TON does

TON's NFT standard, TEP-62, splits an NFT into a collection contract and one contract per item. The
item exposes a single get-method describing itself:

```
get_nft_data() -> (int init?, int index, slice collection_address, slice owner_address, cell individual_content)
```

`init?` is "if not zero, then this NFT is fully initialized and ready for interaction". `index` is
"numerical index of this NFT in the collection". `collection_address` is "address of the smart
contract of the collection to which this NFT belongs".

TEP-62 itself does not specify how to verify that relationship. The verification rule lives in TON's
documentation, and it is explicit that the claim alone is worthless:

> Not every NFT that stores a collection address actually belongs to that collection. Verify that
> the collection returns the item's address for the item's index.

So TON does not leave this gap open. It names the check. What makes the check work on TON is that a
contract address is derived from a hash of its code and its initial data, and the item's initial
data contains its index and its collection. The collection can therefore compute the address that an
item at index `i` must have, and the round trip from index to address is a real binding rather than a
lookup that someone could have written anything into.

## What NEAR has instead

NEAR has no address derivation from code and data, so the TON binding does not port directly. It has
something else that serves the same purpose: a hierarchical account namespace with a
protocol-enforced creation rule. `alice.hosdemo.testnet` can only be created by
`hosdemo.testnet`. Nobody else can produce that name, at any price, by any transaction.

That gives the NEAR equivalent. The item's index is its own account id, read from
`env::current_account_id()` rather than stored, and the parent is a substring of that id. So a
consumer holding the account id already knows which account must have created it, with no view call
and nothing to trust.

The collection round trip is then `nft_token(token_id)`, which NEP-171 already requires. The index is
the account name rather than an integer, which suits a product whose whole point is readable names.

Note what this design deliberately does not do: report the parent as a field. A consumer that has
checked `token_id` against the account it queried can derive the parent itself, so transmitting it
would restate data the caller already holds. Worse, a `parent_id` field invites a consumer to check
the parent and skip the identity check, which is the one comparison that actually catches a contract
lying about which token it is. The namespace is the substrate here, not a value to send.

## The interface

An item implements one additional view. Everything else is NEP-171 as written.

```rust
pub struct NftItemInfo {
    pub init: bool,
    pub collection_id: AccountId,
    pub token_id: String,
    pub owner_id: AccountId,
}

pub fn nft_item_info(&self) -> NftItemInfo;
```

Field by field, against TEP-62's five. Nothing is added to TEP-62's shape and one field is dropped:

| TEP-62 | here | why |
| --- | --- | --- |
| `init?` | `init` | Same meaning. False when the item is parked and has no owner. |
| `index` | `token_id` | The account id, which is this design's index. |
| `collection_address` | `collection_id` | A claim, load-bearing only once the collection confirms it. |
| `owner_address` | `owner_id` | The item is the authority here. |
| `individual_content` | absent | NEP-177 metadata is served by the collection. |

`token_id` stays a `String` rather than an `AccountId` because NEP-171 defines it as a string, and
conformance is worth more than encoding our incidental invariant that it always parses as an account.

Requirements on an implementation:

1. `token_id` MUST be derived from `env::current_account_id()`, never from stored state.
2. `collection_id` MUST NOT confer any authority. Naming a collection must not let an item do
   anything it could not otherwise do, because anyone can name any collection.
3. An item MUST refuse to name itself as its own collection at initialization.
4. `owner_id` MUST come from the state that actually governs the account, not a cached copy.

Point 4 is what makes the item worth asking. Here ownership is the extension set: the accounts
allowed to act as the account. `init` is `extensions.contains(&owner)`, so an item that has been
parked reports itself uninitialised for the same reason it cannot act.

## The check a consumer performs

Ask both sides, refuse unless they agree.

```
item  = nft_item_info() on the account
token = nft_token(item.token_id) on item.collection_id

accept only if:
  item.collection_id == the collection you asked about
  item.token_id      == the account you read it from
  item.init
  token              is not null
  token.token_id     == item.token_id
  token.owner_id     == item.owner_id
```

The collection is pinned by the caller from its own configuration. Taking it from
`item.collection_id` makes the first clause vacuous and defeats the whole check, because an attacker
can then supply both halves: a forged item naming a registry they control, and a registry that
agrees.

Each clause rules out a specific forgery, and none is redundant:

- **The item alone is not enough.** Any contract, deployed at any account, can return an
  `nft_item_info` naming our collection. The collection half is what refuses it.
- **The collection alone is not enough.** `nft_token` is a map lookup. It says the collection
  believes an account is a member; it says nothing about what that account currently does.
- **The identity clause is what makes the rest mean anything.** A contract is free to return any
  `token_id` it likes, so comparing it against the account actually queried is what stops an item
  answering for a token it is not.
- **The owner comparison catches divergence.** The item and the index update in separate receipts.
  If the second fails, they disagree, and a consumer that asked only one of them would act on a
  stale answer.

The attack this closes is the one TON names: an account somewhere claiming membership of a
collection it was never minted into.

## Verify the collection once, then trust every member by its name

The design assumes its end state: the parent account holds no key. Everything below follows from
that, and a deployment that still has a key on the parent is a transient to be finished, not a mode
to design for.

NEAR permits only the parent to create `<label>.<parent>`. With no key on the parent, the only
remaining creator is the parent's own contract. That contract is the registrar, and the registrar
refuses to mint for anyone but the registry it was configured with. The chain is short and every link
is public:

1. the account id ends with `.<parent>`, which only `<parent>` could have produced;
2. `view_access_key_list(<parent>)` is empty, so its minting authority is code and not a person;
3. `<parent>.registry()` names the collection, and the registrar mints only when that collection asks.

Therefore any account under `<parent>` is a name the collection authorised.

The consequence is the interesting part. Steps 2 and 3 are properties of the **collection**, not of
any item, so a consumer checks them **once** and caches the answer. After that, membership of any
name is a string suffix comparison: free, offline, and available to a client that has never heard of
the account before.

This is where NEAR does better than the standard it borrows from. TON binds an item to its collection
through `address = hash(code, data)`. That is strong, but the address is opaque and carries no
provenance, so a TON client must perform a round trip **per item, forever**. A NEAR account id carries
its own provenance, so the cost collapses from once per item to once per collection. TEP-62 cannot
express this, because a TON address does not distinguish who is permitted to produce siblings.

It also gives a client something TEP-62 has no way to offer: the ability to **verify the trust
assumption rather than accept it**. An empty key list is public evidence that nobody can hand-mint a
sibling. On TON there is no equivalent question to ask.

### What the collection round trip is still for

Membership no longer needs it. Ownership does not either, since the item is the account itself and
therefore authoritative. What remains is **consistency**: the item and the registry index are updated
in separate receipts, so a failure between them leaves the index stale while the item is right.

Ask the collection anyway, and refuse on disagreement. Not because you doubt the item, but because a
divergence means the system is in a state nobody designed, and settlement is indexed by the
collection. Fail closed and let an operator repair the index.

## Trust model

The collection is trusted to say who is a member. The item is trusted to say who owns it. Neither is
trusted for the other's answer, and a consumer that accepts a disagreement has no defence against
whichever side is wrong.

The registry contract can rotate an item's ownership through its authority, so a compromised registry
account can move a name; the check detects the resulting divergence but does not prevent the
rotation. That is the residual trust, and removing the registry's key does not remove it, because the
authority is exercised by code the council governs.

**Authenticity is not suitability.** A verified pair says this account is genuinely the name it claims
and is owned by whom it says. It says nothing about whether the lease is live. `init` follows
TEP-62's meaning, that the item is initialised and has an owner, and a name whose lease has expired
but has not yet been reclaimed still reports `init: true`. Anything trading the asset MUST read
`hos_lease` as well, or it will let someone buy a name the authority can reclaim immediately.

**Do not add a code hash check.** It looks like the direct analogue of TON's `hash(code, data)` and it
is the wrong move here. A leased account holds no key, so its code can only change through the
council-gated global contract publish, which means the only threat such a check catches is the
council shipping malicious code, and the council is the trust root. Against that it buys nothing,
while it breaks every consumer during a fleet republish when hashes legitimately differ. More binding,
worse operationally, aimed at a threat outside the model.

## What is proven, and where

Contract behaviour, `contracts/hos-wallet/src/tests.rs`, `mod sharded_item`: an item describes
itself; the reported owner follows a transfer rather than a stale index; `token_id` is read from the
account rather than stored; a parked item reports `init: false`; an item cannot name itself as its
own collection.

End to end against real wasm, `integration/tests/sharded_item.rs`:

- `an_item_and_the_collection_agree_and_a_forgery_does_not`: a minted name passes; a wallet deployed
  off-collection and claiming our registry is refused.
- `a_forgery_created_under_the_real_parent_is_still_refused`: a child created directly under the TLA
  account sits under a genuine parent, so naming separates nothing. The collection clause refuses it,
  which is the case that shows why both halves exist.
- `a_keyless_parent_makes_the_namespace_the_membership_proof`: the same forgery, attempted after the
  parent's key is deleted, cannot be created at all. Note how the previous test is staged, by signing
  as the parent, and that this one removes the ability to stage it. It also asserts the other two
  links, an empty key list and the registrar naming its collection, so the whole chain a consumer
  caches is proven in one place. Names already minted keep verifying.
- `the_pair_refuses_an_item_and_a_collection_that_disagree`: a rotation applied to the item without
  the index catching up, and the pair refuses the split.

Consumer, `packages/contract-client/src/item.ts` in the product repository, with the refusal reasons
enumerated and tested one per clause, including an attacker supplying both halves. Wired into
ownership verification at `apps/server/src/providers/near-ownership.ts`, which pins the collection
from configuration and fails closed if the chain cannot be reached.

## Conformance with TEP-62's stated intent

TEP-62 states its own purpose and its own limitation, so this can be checked against the text rather
than against a reading of it.

**The rationale we inherit.** "'One NFT - one smart contract' simplifies fees calculation and allows
to give gas-consumption guarantees." Every leased name is its own account running its own contract,
so the property TEP-62 is arguing for holds here for the same reason it holds there.

**The drawback we half close, and the half matters.** TEP-62 says plainly: "There is no way to get
current owner of NFT onchain because TON is an asynchronous blockchain." NEAR is asynchronous in the
same way, so an on-chain caller reading an item through a cross-contract call inherits exactly this
limitation: by the time the callback lands, ownership may have moved. This design does not fix that
and must not be described as if it does.

What NEAR permits and TON does not is an off-chain caller pinning both reads to one block height. A
wallet, indexer or marketplace asks the item and the collection at the same height, so the pair
cannot be read either side of a transfer and the answer is atomic. That is the sense in which the
gap closes, and it closes only for off-chain consumers, which is who actually needs it here.

**Deliberate omissions that match theirs.** TEP-62 excludes approvals because "you cannot send the
message 'is there an approval?' because the response may become irrelevant while the response message
is getting to you." We exclude NEP-178 for a related reason from the other end: `nft_transfer` rejects
a set `approval_id` rather than ignoring it, and Intents always passes `None`, so an approval would
add surface nothing uses. TEP-62 also declines to mandate royalties because guaranteeing them means
prohibiting free transfers; commission here is handled in settlement, not by the token, for the same
reason.

**Where the mapping is not one to one.** TEP-62 requires a collection to implement
`get_nft_address_by_index()`, because on TON an index and an address are different things and the
round trip from one to the other is what proves membership. Here the index *is* the address: the
token id is the account id. So the round trip degenerates into `nft_token(token_id)` returning a
token, which NEP-171 already requires. A reader coming from TEP-62 should be told this explicitly,
because they will look for an address-by-index method and not find one.

## Relationship to existing NEPs

This adds one view and changes nothing else. NEP-171 transfer semantics, NEP-177 metadata, NEP-181
enumeration and NEP-297 events are implemented as written and are unaffected. NEP-178 approvals are
deliberately absent: `nft_transfer` rejects a set `approval_id` rather than ignoring it.

A consumer that does not know about `nft_item_info` sees an ordinary NEP-171 collection and loses
nothing it has today. A consumer that does gains the ability to refuse an impostor.
