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
collection it was never minted into. Note what it does not close. If someone holds a parent account's
full access key, they can create a real child of that parent and deploy anything there, so the
account will sit under a genuine parent. Only the collection clause refuses it, because the registry
never minted that name. Naming is a filter; the collection is the security boundary.

## Trust model

The collection is trusted to say who is a member. The item is trusted to say who owns it. Neither is
trusted for the other's answer, and a consumer that accepts a disagreement has no defence against
whichever side is wrong.

Two consequences worth stating plainly. The registry contract can rotate an item's ownership through
its authority, so a compromised registry account can move a name; the check detects the resulting
divergence but does not prevent the rotation. And the parent account's key, while it exists, is a
capability to create genuine children. Removing these keys is what turns both from a human-held
capability into a code-enforced one.

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
- `the_pair_refuses_an_item_and_a_collection_that_disagree`: a rotation applied to the item without
  the index catching up, and the pair refuses the split.

Consumer, `packages/contract-client/src/item.ts` in the product repository, with the refusal reasons
enumerated and tested one per clause, including an attacker supplying both halves. Wired into
ownership verification at `apps/server/src/providers/near-ownership.ts`, which pins the collection
from configuration and fails closed if the chain cannot be reached.

## Relationship to existing NEPs

This adds one view and changes nothing else. NEP-171 transfer semantics, NEP-177 metadata, NEP-181
enumeration and NEP-297 events are implemented as written and are unaffected. NEP-178 approvals are
deliberately absent: `nft_transfer` rejects a set `approval_id` rather than ignoring it.

A consumer that does not know about `nft_item_info` sees an ordinary NEP-171 collection and loses
nothing it has today. A consumer that does gains the ability to refuse an impostor.
