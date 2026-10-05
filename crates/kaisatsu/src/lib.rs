#![doc = include_str!("../README.md")]
#![no_std]
#![forbid(unsafe_code)]
// The verifier parses untrusted bytes on devices that cannot afford to crash: every slice
// access and every arithmetic operation must be visibly checked.
#![deny(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used
)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(test)]
extern crate std;

#[cfg(feature = "base45")]
pub mod base45;
#[cfg(feature = "issuer")]
mod issuer;
mod key;
mod pinpon;
mod ticket;
mod verifier;
pub mod wire;

#[cfg(feature = "issuer")]
pub use issuer::{Claims, IssueError, Issuer};
pub use key::{InvalidKey, KeyId, KeyRing, TrustedKey};
pub use pinpon::{Defect, Pinpon};
pub use ticket::{Extension, ExtensionIter, Extensions, Uuid, VerifiedTicket};
pub use verifier::Verifier;
