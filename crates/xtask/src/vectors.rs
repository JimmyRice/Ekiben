//! Generates the shared KP1 test vectors.
//!
//! Every binding (Rust, C, Swift, Kotlin, Web) runs the same vectors, so a ticket that one
//! platform accepts is accepted by all of them — and the same goes for every rejection.

use std::collections::BTreeMap;

use kaisatsu::wire::{ALGORITHM_ED25519, MAGIC, VERSION, tag};
use kaisatsu::{Claims, Issuer, Uuid, base45};
use serde::Serialize;

use crate::{Result, workspace_root, write_or_check};

const VALID_FROM: u64 = 1_767_225_600; // 2026-01-01T00:00:00Z
const VALID_UNTIL: u64 = 1_767_312_000; // 2026-01-02T00:00:00Z
const ISSUED_AT: u64 = 1_764_547_200; // 2025-12-01T00:00:00Z

pub(crate) fn run(check: bool) -> Result {
    let path = workspace_root().join("spec/test-vectors/v1/vectors.json");
    let mut json = serde_json::to_string_pretty(&build())?;
    json.push('\n');
    write_or_check(&path, &json, check)
}

#[derive(Serialize)]
struct VectorFile {
    protocol: &'static str,
    note: &'static str,
    keys: Vec<KeyPair>,
    vectors: Vec<Vector>,
}

#[derive(Serialize)]
struct KeyPair {
    name: &'static str,
    trusted: bool,
    secret_key: String,
    public_key: String,
    key_id: String,
}

#[derive(Serialize)]
struct Vector {
    name: &'static str,
    description: &'static str,
    ticket: String,
    base45: String,
    /// `"Ok"` or the name of the expected `Pinpon` variant.
    expect: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    claims: Option<ExpectedClaims>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    time_checks: Vec<TimeCheck>,
}

#[derive(Serialize)]
struct ExpectedClaims {
    issuer: String,
    event_id: String,
    ticket_id: String,
    ticket_type_id: String,
    valid_from: u64,
    valid_until: u64,
    issued_at: u64,
    extensions: Vec<ExpectedExtension>,
}

#[derive(Serialize)]
struct ExpectedExtension {
    tag: u8,
    value: String,
}

#[derive(Serialize)]
struct TimeCheck {
    now: u64,
    expect: &'static str,
}

fn trusted_secret() -> [u8; 32] {
    std::array::from_fn(|i| u8::try_from(i + 1).unwrap_or(0))
}

fn untrusted_secret() -> [u8; 32] {
    std::array::from_fn(|i| u8::try_from(0xA0 + i).unwrap_or(0))
}

fn key_pair(name: &'static str, secret: [u8; 32], trusted: bool) -> KeyPair {
    let issuer = Issuer::from_secret_key(&secret);
    KeyPair {
        name,
        trusted,
        secret_key: hex::encode(secret),
        public_key: hex::encode(issuer.public_key()),
        key_id: issuer.key_id().to_string(),
    }
}

fn uuid(text: &str) -> Uuid {
    let mut bytes = [0; 16];
    hex::decode_to_slice(text.replace('-', ""), &mut bytes).unwrap_or_default();
    Uuid::from_bytes(bytes)
}

fn base_claims() -> Claims {
    Claims {
        issuer: "kippu.example.org".to_owned(),
        event_id: uuid("01994a3c-7d00-7000-8000-000000000001"),
        ticket_id: uuid("01994a3c-7d00-7000-8000-0000000000a1"),
        ticket_type_id: uuid("01994a3c-7d00-7000-8000-0000000000b1"),
        valid_from: VALID_FROM,
        valid_until: VALID_UNTIL,
        issued_at: ISSUED_AT,
        extensions: BTreeMap::new(),
    }
}

fn expected(claims: &Claims) -> ExpectedClaims {
    ExpectedClaims {
        issuer: claims.issuer.clone(),
        event_id: claims.event_id.to_string(),
        ticket_id: claims.ticket_id.to_string(),
        ticket_type_id: claims.ticket_type_id.to_string(),
        valid_from: claims.valid_from,
        valid_until: claims.valid_until,
        issued_at: claims.issued_at,
        extensions: claims
            .extensions
            .iter()
            .map(|(&tag, value)| ExpectedExtension {
                tag,
                value: hex::encode(value),
            })
            .collect(),
    }
}

/// Builds `header ‖ claims` by hand, for tickets the `Issuer` would refuse to produce.
struct RawTicket {
    bytes: Vec<u8>,
}

impl RawTicket {
    fn new(issuer: &Issuer) -> Self {
        Self::with_header(VERSION, ALGORITHM_ED25519, issuer)
    }

    fn with_header(version: u8, algorithm: u8, issuer: &Issuer) -> Self {
        let mut bytes = MAGIC.to_vec();
        bytes.extend([version, algorithm]);
        bytes.extend(issuer.key_id().to_bytes());
        Self { bytes }
    }

    fn claim(mut self, tag: u8, value: &[u8]) -> Self {
        self.bytes.push(tag);
        self.bytes
            .push(u8::try_from(value.len()).unwrap_or(u8::MAX));
        self.bytes.extend_from_slice(value);
        self
    }

    fn raw(mut self, bytes: &[u8]) -> Self {
        self.bytes.extend_from_slice(bytes);
        self
    }

    /// Every required claim except those in `skip`, in ascending order.
    fn required_claims(self, skip: &[u8]) -> Self {
        let claims = base_claims();
        let all: [(u8, Vec<u8>); 7] = [
            (tag::ISSUER, claims.issuer.into_bytes()),
            (tag::EVENT_ID, claims.event_id.to_bytes().to_vec()),
            (tag::TICKET_ID, claims.ticket_id.to_bytes().to_vec()),
            (
                tag::TICKET_TYPE_ID,
                claims.ticket_type_id.to_bytes().to_vec(),
            ),
            (tag::VALID_FROM, claims.valid_from.to_be_bytes().to_vec()),
            (tag::VALID_UNTIL, claims.valid_until.to_be_bytes().to_vec()),
            (tag::ISSUED_AT, claims.issued_at.to_be_bytes().to_vec()),
        ];
        all.into_iter()
            .filter(|(tag, _)| !skip.contains(tag))
            .fold(self, |ticket, (tag, value)| ticket.claim(tag, &value))
    }

    fn sign(self, issuer: &Issuer) -> Vec<u8> {
        issuer.sign_unchecked(self.bytes)
    }
}

fn vector(
    name: &'static str,
    description: &'static str,
    ticket: &[u8],
    expect: &'static str,
) -> Vector {
    Vector {
        name,
        description,
        base45: base45::encode(ticket),
        ticket: hex::encode(ticket),
        expect,
        claims: None,
        time_checks: Vec::new(),
    }
}

fn issue(issuer: &Issuer, claims: &Claims) -> Vec<u8> {
    issuer.issue(claims).unwrap_or_default()
}

fn build() -> VectorFile {
    let trusted = Issuer::from_secret_key(&trusted_secret());
    let untrusted = Issuer::from_secret_key(&untrusted_secret());
    let mut vectors = accepted(&trusted);
    vectors.extend(rejected(&trusted, &untrusted));

    VectorFile {
        protocol: "KP1",
        note: "Generated by `cargo xtask vectors`; do not edit. Verify with the keys marked trusted.",
        keys: vec![
            key_pair("trusted", trusted_secret(), true),
            key_pair("untrusted", untrusted_secret(), false),
        ],
        vectors,
    }
}

/// Tickets every verifier must accept.
fn accepted(trusted: &Issuer) -> Vec<Vector> {
    let minimal = base_claims();
    let mut with_extensions = base_claims();
    with_extensions.extensions = BTreeMap::from([
        (0x80, b"HALL-A".to_vec()),
        (0x81, vec![0x01]),
        (0xC0, vec![0xAB; 200]), // long enough to need a two-byte length
    ]);

    let mut vectors = Vec::new();

    let mut valid = vector(
        "valid-minimal",
        "A ticket with only the required claims.",
        &issue(trusted, &minimal),
        "Ok",
    );
    valid.claims = Some(expected(&minimal));
    valid.time_checks = vec![
        TimeCheck {
            now: VALID_FROM - 1,
            expect: "NotYetValid",
        },
        TimeCheck {
            now: VALID_FROM,
            expect: "Ok",
        },
        TimeCheck {
            now: VALID_UNTIL - 1,
            expect: "Ok",
        },
        TimeCheck {
            now: VALID_UNTIL,
            expect: "Expired",
        },
    ];
    vectors.push(valid);

    let mut extended = vector(
        "valid-with-extensions",
        "Application-defined extensions are returned verbatim, in tag order.",
        &issue(trusted, &with_extensions),
        "Ok",
    );
    extended.claims = Some(expected(&with_extensions));
    vectors.push(extended);

    let mut forward_compatible = vector(
        "valid-with-reserved-optional-claim",
        "Unknown claims in 0x40..=0x7F come from a future revision and are ignored.",
        &RawTicket::new(trusted)
            .required_claims(&[])
            .claim(0x40, b"future")
            .sign(trusted),
        "Ok",
    );
    forward_compatible.claims = Some(expected(&minimal));
    vectors.push(forward_compatible);
    vectors
}

/// Tickets every verifier must reject, with the reason.
#[expect(clippy::too_many_lines, reason = "a flat list of cases reads best")]
fn rejected(trusted: &Issuer, untrusted: &Issuer) -> Vec<Vector> {
    let minimal = base_claims();
    let mut vectors = Vec::new();

    let mut bad_signature = issue(trusted, &minimal);
    if let Some(last) = bad_signature.last_mut() {
        *last ^= 0x01;
    }
    vectors.push(vector(
        "bad-signature",
        "The last signature bit is flipped.",
        &bad_signature,
        "BadSignature",
    ));

    let mut tampered = issue(trusted, &minimal);
    if let Some(byte) = tampered.get_mut(40) {
        *byte ^= 0x01; // inside the event id
    }
    vectors.push(vector(
        "tampered-claims",
        "A claim byte is changed after signing.",
        &tampered,
        "BadSignature",
    ));

    vectors.push(vector(
        "unknown-key",
        "Signed by a key the verifier does not trust.",
        &issue(untrusted, &minimal),
        "UnknownKey",
    ));

    let mut bad_magic = issue(trusted, &minimal);
    if let Some(byte) = bad_magic.get_mut(1) {
        *byte = b'Q';
    }
    vectors.push(vector(
        "bad-magic",
        "Does not start with \"KP\".",
        &bad_magic,
        "Malformed",
    ));

    vectors.push(vector(
        "unsupported-version",
        "Protocol version 2.",
        &RawTicket::with_header(2, ALGORITHM_ED25519, trusted)
            .required_claims(&[])
            .sign(trusted),
        "UnsupportedVersion",
    ));
    vectors.push(vector(
        "unsupported-algorithm",
        "Algorithm 2.",
        &RawTicket::with_header(VERSION, 2, trusted)
            .required_claims(&[])
            .sign(trusted),
        "UnsupportedAlgorithm",
    ));
    vectors.push(vector(
        "too-short",
        "Shorter than a header plus a signature.",
        &[b'K', b'P', 1, 1],
        "Malformed",
    ));
    vectors.push(vector(
        "claims-out-of-order",
        "Signed, but tags are not strictly ascending.",
        &RawTicket::new(trusted)
            .claim(tag::EVENT_ID, &minimal.event_id.to_bytes())
            .claim(tag::ISSUER, minimal.issuer.as_bytes())
            .sign(trusted),
        "Malformed",
    ));
    vectors.push(vector(
        "unknown-critical-claim",
        "Signed, but carries claim 0x08, which version 1 reserves as critical.",
        &RawTicket::new(trusted)
            .required_claims(&[])
            .claim(0x08, b"?")
            .sign(trusted),
        "Malformed",
    ));
    vectors.push(vector(
        "missing-claim",
        "Signed, but has no issued_at claim.",
        &RawTicket::new(trusted)
            .required_claims(&[tag::ISSUED_AT])
            .sign(trusted),
        "Malformed",
    ));
    vectors.push(vector(
        "non-canonical-length",
        "Signed, but a length is encoded in two bytes where one suffices.",
        &RawTicket::new(trusted)
            .raw(&[tag::ISSUER, 0x81, 0x00, b'k'])
            .sign(trusted),
        "Malformed",
    ));
    vectors.push(vector(
        "truncated-claim",
        "Signed, but the last claim's length runs past the end.",
        &RawTicket::new(trusted)
            .required_claims(&[])
            .raw(&[0x80, 0x10, 0x00])
            .sign(trusted),
        "Malformed",
    ));

    vectors
}
