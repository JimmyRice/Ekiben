//! Arbitrary bytes, with and without a valid header, must never panic or verify.
#![no_main]

use kaisatsu::{Issuer, Verifier};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let issuer = Issuer::from_secret_key(&[7; 32]);
    let verifier = Verifier::new(issuer.trusted_key());
    assert!(verifier.verify(data).is_err());

    let mut ticket = b"KP\x01\x01".to_vec();
    ticket.extend_from_slice(&issuer.key_id().to_bytes());
    ticket.extend_from_slice(data);
    let _ = verifier.verify(&ticket);

    // Signed garbage reaches the claims parser.
    let signed = issuer.sign_unchecked(ticket[..ticket.len().min(1900)].to_vec());
    let _ = verifier.verify(&signed);
});
