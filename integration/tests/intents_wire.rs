use defuse_core::intents::DefuseIntents;
use defuse_core::payload::multi::MultiPayload;
use defuse_core::payload::DefusePayload;
use defuse_core::Timestamp;
use serde_json::json;

const REGISTRY: &str = "registry.hosdemo.testnet";
const NAME: &str = "alice.hosdemo.testnet";
const SELLER: &str = "bob.testnet";
const BUYER: &str = "carol.testnet";

fn name_token() -> String {
    format!("nep171:{REGISTRY}:{NAME}")
}

#[test]
fn a_sale_diff_deserializes_as_the_verifier_reads_it() {
    let raw = json!({
        "intent": "token_diff",
        "diff": {
            name_token(): "-1",
            "nep141:wrap.near": "50000000000000000000000000",
        },
    });
    let intents: DefuseIntents = serde_json::from_value(json!({ "intents": [raw] }))
        .expect("the verifier must accept the sale diff we build");
    assert_eq!(intents.intents.len(), 1);
}

#[test]
fn an_nft_withdraw_deserializes_as_the_verifier_reads_it() {
    let raw = json!({
        "intent": "nft_withdraw",
        "token": REGISTRY,
        "receiver_id": BUYER,
        "token_id": NAME,
    });
    let intents: DefuseIntents = serde_json::from_value(json!({ "intents": [raw] }))
        .expect("the verifier must accept the withdrawal we build");
    assert_eq!(intents.intents.len(), 1);
}

#[test]
fn the_signed_message_body_deserializes_with_an_rfc3339_deadline() {
    let body = json!({
        "signer_id": SELLER,
        "verifying_contract": "intents.near",
        "deadline": "2026-08-16T00:00:00Z",
        "nonce": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        "intents": [{
            "intent": "token_diff",
            "diff": { name_token(): "-1", "nep141:wrap.near": "1" },
        }],
    });
    let payload: DefusePayload<DefuseIntents> =
        serde_json::from_value(body).expect("the verifier must accept our signed message body");
    assert_eq!(payload.signer_id.as_str(), SELLER);
    assert_eq!(payload.message.intents.len(), 1);
}

#[test]
fn a_deadline_in_nanoseconds_is_refused_so_the_wrong_unit_cannot_reach_mainnet() {
    let body = json!({
        "signer_id": SELLER,
        "verifying_contract": "intents.near",
        "deadline": "1786681751917522625",
        "nonce": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        "intents": [],
    });
    assert!(
        serde_json::from_value::<DefusePayload<DefuseIntents>>(body).is_err(),
        "deadline is rfc3339, and a nanosecond timestamp must not silently parse"
    );
}

#[test]
fn the_nep413_envelope_deserializes_as_the_verifier_reads_it() {
    let envelope = json!({
        "standard": "nep413",
        "payload": {
            "message": "{}",
            "nonce": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "recipient": "intents.near",
        },
        "public_key": "ed25519:DcA2MzgpJbrUATQLLceocVckhhAqrkingax4oJ9kZ847",
        "signature": "ed25519:3s1DvfPyG5FKWGpwKDbrCM4k6HFjNvSGRQGYVvrGuTyKCEmXHFxYFhsMy2GAsBEB1FvpjbKKrTFCA9BbSHM2Nqjs",
    });
    let _: MultiPayload = serde_json::from_value(envelope)
        .expect("the verifier must accept the envelope our wallet signature assembles");
}

#[test]
fn a_timestamp_renders_as_rfc3339_not_as_a_number() {
    let rendered = serde_json::to_string(&Timestamp::UNIX_EPOCH).unwrap();
    assert!(
        rendered.starts_with('"') && rendered.contains('T'),
        "deadline must be an rfc3339 string, got {rendered}"
    );
}

#[test]
fn the_deadline_javascript_produces_is_accepted() {
    let body = json!({
        "signer_id": SELLER,
        "verifying_contract": "intents.near",
        "deadline": "2026-08-16T00:00:00.000Z",
        "nonce": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        "intents": [],
    });
    serde_json::from_value::<DefusePayload<DefuseIntents>>(body)
        .expect("toISOString emits fractional seconds and the verifier must take them");
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn the_versioned_nonce_we_build_off_chain_decodes_on_chain() {
    use base64::Engine;
    use defuse_core::{ExpirableNonce, Salt, SaltedNonce, VersionedNonce};

    let salt: Salt = "01020304".parse().expect("a salt is hex on the wire");
    let deadline = Timestamp::from_nanos(1_786_000_000_000_000_000).unwrap();
    let built = VersionedNonce::V1(SaltedNonce {
        salt,
        nonce: ExpirableNonce {
            deadline,
            nonce: [7u8; 15],
        },
    });

    let bytes: [u8; 32] = built.clone().into();
    println!(
        "NONCE_VECTOR_BASE64={}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    );
    println!("NONCE_VECTOR_HEX={}", hex_of(&bytes));

    assert_eq!(
        VersionedNonce::maybe_from(bytes),
        Some(built),
        "a nonce built off chain must decode to the same salt and deadline on chain"
    );
}

#[test]
fn a_random_nonce_is_read_as_legacy_and_carries_no_expiry() {
    use defuse_core::VersionedNonce;

    assert!(
        VersionedNonce::maybe_from([0x11u8; 32]).is_none(),
        "random bytes are legacy nonces, which the protocol is deprecating"
    );
}

#[test]
fn what_a_wallet_actually_signs_is_the_nep413_defuse_message() {
    use defuse_core::payload::nep413::Nep413DefuseMessage;

    let signed = json!({
        "signer_id": SELLER,
        "deadline": "2026-08-16T00:00:00.000Z",
        "intents": [{
            "intent": "token_diff",
            "diff": { name_token(): "-1", "nep141:wrap.near": "1" },
        }],
    });
    let message: Nep413DefuseMessage<DefuseIntents> = serde_json::from_value(signed)
        .expect("the body a wallet signs carries signer_id, deadline and the intents only");
    assert_eq!(message.signer_id.as_str(), SELLER);
    assert_eq!(message.message.intents.len(), 1);
}

#[test]
fn the_signed_body_must_not_carry_the_contract_or_the_nonce() {
    use defuse_core::payload::nep413::Nep413DefuseMessage;

    let with_envelope_fields = json!({
        "signer_id": SELLER,
        "verifying_contract": "intents.near",
        "nonce": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
        "deadline": "2026-08-16T00:00:00.000Z",
        "intents": [],
    });
    let message: Nep413DefuseMessage<DefuseIntents> =
        serde_json::from_value(with_envelope_fields).unwrap();
    let round_tripped = serde_json::to_value(&message).unwrap();
    assert!(
        round_tripped.get("verifying_contract").is_none() && round_tripped.get("nonce").is_none(),
        "recipient and nonce live in the nep413 envelope, not in the signed body"
    );
}
