//! Runs the shared KP1 test vectors through the UniFFI `Verifier`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration tests may unwrap"
)]

use kaisatsu_uniffi::{KaisatsuError, Verifier};
use serde_json::Value;

fn vectors() -> Value {
    serde_json::from_str(include_str!(
        "../../../../spec/test-vectors/v1/vectors.json"
    ))
    .unwrap()
}

fn kind(error: &KaisatsuError) -> &'static str {
    match error {
        KaisatsuError::Malformed(_) => "Malformed",
        KaisatsuError::UnsupportedVersion(_) => "UnsupportedVersion",
        KaisatsuError::UnsupportedAlgorithm(_) => "UnsupportedAlgorithm",
        KaisatsuError::UnknownKey(_) => "UnknownKey",
        KaisatsuError::BadSignature(_) => "BadSignature",
        KaisatsuError::NotYetValid(_) => "NotYetValid",
        KaisatsuError::Expired(_) => "Expired",
        KaisatsuError::InvalidKey(_) => "InvalidKey",
    }
}

#[test]
fn every_vector_gives_the_expected_result() {
    let vectors = vectors();
    let key = vectors["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|key| key["trusted"] == true)
        .unwrap();
    let verifier = Verifier::new(vec![
        hex::decode(key["public_key"].as_str().unwrap()).unwrap(),
    ])
    .unwrap();

    for vector in vectors["vectors"].as_array().unwrap() {
        let name = vector["name"].as_str().unwrap();
        let ticket = hex::decode(vector["ticket"].as_str().unwrap()).unwrap();
        let claims = &vector["claims"];
        let checks = vector["time_checks"].as_array().map_or_else(
            || {
                vec![(
                    claims["valid_from"].as_u64().unwrap_or(0),
                    vector["expect"].clone(),
                )]
            },
            |checks| {
                checks
                    .iter()
                    .map(|check| (check["now"].as_u64().unwrap(), check["expect"].clone()))
                    .collect()
            },
        );
        for (now, expect) in checks {
            let result = verifier.verify(ticket.clone(), now);
            match (expect.as_str().unwrap(), result) {
                ("Ok", Ok(ticket)) => {
                    assert_eq!(ticket.issuer, claims["issuer"], "{name}");
                    assert_eq!(ticket.event_id, claims["event_id"], "{name}");
                    assert_eq!(ticket.ticket_id, claims["ticket_id"], "{name}");
                    assert_eq!(ticket.ticket_type_id, claims["ticket_type_id"], "{name}");
                    assert_eq!(ticket.valid_from, claims["valid_from"], "{name}");
                    assert_eq!(ticket.valid_until, claims["valid_until"], "{name}");
                    assert_eq!(ticket.issued_at, claims["issued_at"], "{name}");
                    assert_eq!(
                        ticket.extensions.len(),
                        claims["extensions"].as_array().unwrap().len(),
                        "{name}"
                    );
                }
                (expected, Err(error)) => assert_eq!(kind(&error), expected, "{name} at {now}"),
                (expected, Ok(_)) => panic!("{name} at {now}: expected {expected}, got Ok"),
            }
        }
    }
}

#[test]
fn unusable_keys_are_rejected_up_front() {
    for key in [vec![0; 31], vec![0; 32]] {
        assert!(matches!(
            Verifier::new(vec![key]),
            Err(KaisatsuError::InvalidKey(_))
        ));
    }
}
