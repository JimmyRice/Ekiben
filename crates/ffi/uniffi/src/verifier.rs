use kaisatsu::TrustedKey;

use crate::{KaisatsuError, Ticket};

/// Checks scanned tickets against a set of trusted keys.
///
/// Create it once, at startup, and keep it: the keys are parsed here, not on every scan.
#[derive(uniffi::Object)]
pub struct Verifier {
    keys: Vec<TrustedKey>,
}

#[uniffi::export]
#[expect(
    clippy::needless_pass_by_value,
    reason = "UniFFI hands the generated glue owned arguments"
)]
impl Verifier {
    /// Trusts the given 32-byte Ed25519 public keys, as published at
    /// `/.well-known/kippu/ticket-keys`. Fails with `InvalidKey` if one is unusable.
    #[uniffi::constructor]
    pub fn new(public_keys: Vec<Vec<u8>>) -> Result<Self, KaisatsuError> {
        let keys = public_keys
            .iter()
            .map(|bytes| {
                <&[u8; 32]>::try_from(bytes.as_slice())
                    .ok()
                    .and_then(|key| TrustedKey::from_bytes(key).ok())
                    .ok_or_else(|| KaisatsuError::InvalidKey("invalid public key".into()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { keys })
    }

    /// Verifies a binary ticket and checks that `now` (Unix seconds) lies within its validity
    /// window. Returns the claims, or the reason for the *pinpōn* 🔔.
    pub fn verify(&self, ticket: Vec<u8>, now: u64) -> Result<Ticket, KaisatsuError> {
        let verified = kaisatsu::Verifier::new(&self.keys).verify(&ticket)?;
        verified.check_time(now)?;
        Ok(Ticket::from(&verified))
    }
}
