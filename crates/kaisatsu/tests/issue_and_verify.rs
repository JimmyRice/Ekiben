//! Round trips through `Issuer` and `Verifier`.

use std::collections::BTreeMap;

use kaisatsu::{Claims, Defect, IssueError, Issuer, Pinpon, Uuid, Verifier};
use proptest::prelude::*;

const VALID_FROM: u64 = 1_767_225_600; // 2026-01-01T00:00:00Z
const VALID_UNTIL: u64 = VALID_FROM + 86_400;

fn issuer() -> Issuer {
    Issuer::from_secret_key(&[7; 32])
}

fn claims() -> Claims {
    Claims {
        issuer: "kippu.example.org".to_owned(),
        event_id: Uuid::from_bytes([1; 16]),
        ticket_id: Uuid::from_bytes([2; 16]),
        ticket_type_id: Uuid::from_bytes([3; 16]),
        valid_from: VALID_FROM,
        valid_until: VALID_UNTIL,
        issued_at: VALID_FROM - 3_600,
        extensions: BTreeMap::from([(0x80, b"A-12".to_vec()), (0x81, vec![1])]),
    }
}

#[test]
fn issued_tickets_verify_and_expose_every_claim() {
    let issuer = issuer();
    let bytes = issuer.issue(&claims()).unwrap();
    let ticket = Verifier::new(issuer.trusted_key()).verify(&bytes).unwrap();

    assert_eq!(ticket.version(), 1);
    assert_eq!(ticket.key_id(), issuer.key_id());
    assert_eq!(ticket.issuer(), "kippu.example.org");
    assert_eq!(ticket.event_id(), Uuid::from_bytes([1; 16]));
    assert_eq!(ticket.ticket_id(), Uuid::from_bytes([2; 16]));
    assert_eq!(ticket.ticket_type_id(), Uuid::from_bytes([3; 16]));
    assert_eq!(ticket.valid_from(), VALID_FROM);
    assert_eq!(ticket.valid_until(), VALID_UNTIL);
    assert_eq!(ticket.issued_at(), VALID_FROM - 3_600);
    assert_eq!(ticket.extensions().get(0x80), Some(&b"A-12"[..]));
    assert_eq!(ticket.extensions().get(0x81), Some(&[1][..]));
    assert_eq!(ticket.extensions().get(0x82), None);
    assert_eq!(ticket.extensions().iter().count(), 2);
}

#[test]
fn issuing_is_deterministic() {
    assert_eq!(issuer().issue(&claims()), issuer().issue(&claims()));
}

#[test]
fn time_checks_use_a_half_open_window() {
    let issuer = issuer();
    let bytes = issuer.issue(&claims()).unwrap();
    let ticket = Verifier::new(issuer.trusted_key()).verify(&bytes).unwrap();

    assert_eq!(ticket.check_time(VALID_FROM - 1), Err(Pinpon::NotYetValid));
    assert_eq!(ticket.check_time(VALID_FROM), Ok(()));
    assert_eq!(ticket.check_time(VALID_UNTIL - 1), Ok(()));
    assert_eq!(ticket.check_time(VALID_UNTIL), Err(Pinpon::Expired));
}

#[test]
fn a_ticket_from_another_key_is_unknown() {
    let bytes = issuer().issue(&claims()).unwrap();
    let stranger = Issuer::from_secret_key(&[8; 32]);
    let result = Verifier::new(stranger.trusted_key()).verify(&bytes);
    assert_eq!(result, Err(Pinpon::UnknownKey(issuer().key_id())));
}

#[test]
fn verifiers_accept_any_key_in_the_ring() {
    let old = Issuer::from_secret_key(&[9; 32]);
    let current = issuer();
    let verifier = Verifier::new([old.trusted_key(), current.trusted_key()]);
    assert!(verifier.verify(&old.issue(&claims()).unwrap()).is_ok());
    assert!(verifier.verify(&current.issue(&claims()).unwrap()).is_ok());
}

#[test]
fn a_bad_signature_rings_the_bell() {
    let issuer = issuer();
    let mut bytes = issuer.issue(&claims()).unwrap();
    *bytes.last_mut().unwrap() ^= 1;

    let pinpon = Verifier::new(issuer.trusted_key())
        .verify(&bytes)
        .unwrap_err();
    assert_eq!(pinpon, Pinpon::BadSignature);
    assert_eq!(pinpon.to_string(), "ピンポーン🔔 BadSignature");
}

#[test]
#[cfg(feature = "base45")]
fn base45_text_verifies_like_the_bytes() {
    let issuer = issuer();
    let bytes = issuer.issue(&claims()).unwrap();
    let text = kaisatsu::base45::encode(&bytes);
    assert!(
        text.bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || b" $%*+-./:".contains(&c))
    );

    let mut buffer = [0; kaisatsu::wire::MAX_TICKET_LEN];
    let ticket = Verifier::new(issuer.trusted_key())
        .verify_base45(&text, &mut buffer)
        .unwrap();
    assert_eq!(ticket.ticket_id(), Uuid::from_bytes([2; 16]));
}

#[test]
fn issuing_rejects_invalid_claims() {
    let issuer = issuer();
    let reject = |edit: fn(&mut Claims)| {
        let mut claims = claims();
        edit(&mut claims);
        issuer.issue(&claims).unwrap_err()
    };
    assert_eq!(reject(|c| c.issuer.clear()), IssueError::InvalidIssuer);
    assert_eq!(
        reject(|c| c.issuer = "x".repeat(65)),
        IssueError::InvalidIssuer
    );
    assert_eq!(
        reject(|c| c.valid_until = c.valid_from),
        IssueError::EmptyValidity
    );
    assert_eq!(
        reject(|c| {
            c.extensions.insert(0x7F, vec![]);
        }),
        IssueError::ReservedTag(0x7F)
    );
    assert_eq!(
        reject(|c| {
            c.extensions.insert(0x90, vec![0; 3000]);
        }),
        IssueError::TicketTooLong
    );
}

#[test]
fn long_claims_use_two_byte_lengths() {
    let issuer = issuer();
    let mut claims = claims();
    claims.extensions.insert(0xA0, vec![0xEE; 300]);
    let bytes = issuer.issue(&claims).unwrap();
    let ticket = Verifier::new(issuer.trusted_key()).verify(&bytes).unwrap();
    assert_eq!(ticket.extensions().get(0xA0).map(<[u8]>::len), Some(300));
}

proptest! {
    /// Changing any single bit of a ticket must be detected — and must never panic.
    #[test]
    fn every_bit_flip_is_rejected(position in 0usize..4096, bit in 0u8..8) {
        let issuer = issuer();
        let mut bytes = issuer.issue(&claims()).unwrap();
        let position = position % bytes.len();
        bytes[position] ^= 1 << bit;
        prop_assert!(Verifier::new(issuer.trusted_key()).verify(&bytes).is_err());
    }

    /// Arbitrary input never panics and is never accepted.
    #[test]
    fn arbitrary_bytes_are_rejected(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        prop_assert!(Verifier::new(issuer().trusted_key()).verify(&bytes).is_err());
    }

    /// Arbitrary input behind a valid-looking header never panics.
    #[test]
    fn arbitrary_bodies_are_rejected(body in proptest::collection::vec(any::<u8>(), 0..512)) {
        let issuer = issuer();
        let mut bytes = b"KP\x01\x01".to_vec();
        bytes.extend_from_slice(&issuer.key_id().to_bytes());
        bytes.extend_from_slice(&body);
        let result = Verifier::new(issuer.trusted_key()).verify(&bytes);
        prop_assert!(matches!(result, Err(Pinpon::BadSignature | Pinpon::Malformed(Defect::TooShort))));
    }
}
