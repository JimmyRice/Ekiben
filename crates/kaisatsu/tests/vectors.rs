//! Runs the shared KP1 test vectors in `spec/test-vectors/v1`, as every binding does.

use kaisatsu::{TrustedKey, VerifiedTicket, Verifier};
use serde::Deserialize;

#[derive(Deserialize)]
struct VectorFile {
    keys: Vec<Key>,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Key {
    trusted: bool,
    public_key: String,
}

#[derive(Deserialize)]
struct Vector {
    name: String,
    ticket: String,
    #[cfg_attr(
        not(feature = "base45"),
        expect(dead_code, reason = "only checked with base45")
    )]
    base45: String,
    expect: String,
    claims: Option<Claims>,
    #[serde(default)]
    time_checks: Vec<TimeCheck>,
}

#[derive(Deserialize)]
struct Claims {
    issuer: String,
    event_id: String,
    ticket_id: String,
    ticket_type_id: String,
    valid_from: u64,
    valid_until: u64,
    issued_at: u64,
    extensions: Vec<Extension>,
}

#[derive(Deserialize)]
struct Extension {
    tag: u8,
    value: String,
}

#[derive(Deserialize)]
struct TimeCheck {
    now: u64,
    expect: String,
}

const VECTORS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../spec/test-vectors/v1/vectors.json"
);

fn outcome<T>(result: Result<T, kaisatsu::Pinpon>) -> String {
    result.map_or_else(|pinpon| pinpon.name().to_owned(), |_| "Ok".to_owned())
}

fn assert_claims(name: &str, ticket: &VerifiedTicket<'_>, expected: &Claims) {
    assert_eq!(ticket.issuer(), expected.issuer, "{name}");
    assert_eq!(ticket.event_id().to_string(), expected.event_id, "{name}");
    assert_eq!(ticket.ticket_id().to_string(), expected.ticket_id, "{name}");
    assert_eq!(
        ticket.ticket_type_id().to_string(),
        expected.ticket_type_id,
        "{name}"
    );
    assert_eq!(ticket.valid_from(), expected.valid_from, "{name}");
    assert_eq!(ticket.valid_until(), expected.valid_until, "{name}");
    assert_eq!(ticket.issued_at(), expected.issued_at, "{name}");
    let extensions: Vec<(u8, String)> = ticket
        .extensions()
        .iter()
        .map(|extension| (extension.tag, hex::encode(extension.value)))
        .collect();
    let expected: Vec<(u8, String)> = expected
        .extensions
        .iter()
        .map(|e| (e.tag, e.value.clone()))
        .collect();
    assert_eq!(extensions, expected, "{name}");
}

#[test]
fn every_vector_has_the_expected_outcome() {
    let file: VectorFile =
        serde_json::from_str(&std::fs::read_to_string(VECTORS).unwrap()).unwrap();
    let keys: Vec<TrustedKey> = file
        .keys
        .iter()
        .filter(|key| key.trusted)
        .map(|key| {
            let bytes: [u8; 32] = hex::decode(&key.public_key).unwrap().try_into().unwrap();
            TrustedKey::from_bytes(&bytes).unwrap()
        })
        .collect();
    let verifier = Verifier::new(keys.as_slice());

    for vector in &file.vectors {
        let bytes = hex::decode(&vector.ticket).unwrap();
        let result = verifier.verify(&bytes);
        assert_eq!(outcome(result), vector.expect, "{}", vector.name);

        #[cfg(feature = "base45")]
        {
            let mut buffer = [0; kaisatsu::wire::MAX_TICKET_LEN];
            let from_text = verifier.verify_base45(&vector.base45, &mut buffer);
            assert_eq!(
                outcome(from_text),
                vector.expect,
                "{} (base45)",
                vector.name
            );
        }

        if let (Ok(ticket), Some(claims)) = (result, &vector.claims) {
            assert_claims(&vector.name, &ticket, claims);
            for check in &vector.time_checks {
                assert_eq!(
                    outcome(ticket.check_time(check.now)),
                    check.expect,
                    "{}",
                    vector.name
                );
            }
        }
    }
}
