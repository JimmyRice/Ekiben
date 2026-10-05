//! A pretend payment provider, for trying Kippu by hand.
//!
//! Real attestors wrap Stripe, Alipay, a cash desk… This one just signs a "paid" attestation
//! for a reservation and prints the `curl` command that delivers it:
//!
//! ```text
//! cargo run -p kippu-server --example attestor_sim -- \
//!     --attestor <attestor id> --key attestor.key --reservation <reservation id> \
//!     --amount 2500 --currency JPY | sh
//! ```
//!
//! Register the attestor first (`POST /v1/admin/attestors`) with the public key from
//! `kippu keygen`, and add it to the sale's `accepted_attestors`.
#![allow(clippy::print_stdout, reason = "printing the command is the point")]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use kippu_core::keys::parse_signing_key;
use kippu_core::modules::payments::signature::sign_request;
use kippu_domain::{AttestorId, ReservationId};
use secrecy::SecretString;

/// Signs a payment attestation and prints the curl command that delivers it.
#[derive(Parser)]
struct Args {
    /// Kippu's base URL.
    #[arg(long, default_value = "http://localhost:8080")]
    base_url: String,
    /// The attestor's id.
    #[arg(long)]
    attestor: AttestorId,
    /// File with the attestor's private key (base64 seed or PKCS#8 PEM).
    #[arg(long)]
    key: PathBuf,
    /// The key's id as registered.
    #[arg(long, default_value = "k1")]
    key_id: String,
    /// The reservation that was paid.
    #[arg(long)]
    reservation: ReservationId,
    /// Amount paid, in minor units.
    #[arg(long)]
    amount: i64,
    /// ISO 4217 currency.
    #[arg(long, default_value = "JPY")]
    currency: String,
    /// The payment's reference (defaults to a fresh one).
    #[arg(long)]
    attestation_id: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let key = parse_signing_key(
        "--key",
        &SecretString::from(std::fs::read_to_string(&args.key)?),
    )?;
    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())?;
    let attestation_id = args.attestation_id.unwrap_or_else(|| format!("sim-{now}"));
    let body = format!(
        r#"{{"attestation_id":"{attestation_id}","outcome":"paid","reservation_id":"{}","amount":{{"amount_minor":{},"currency":"{}"}}}}"#,
        args.reservation, args.amount, args.currency
    );
    let path = "/v1/payment-attestations";
    let signature = sign_request(
        &key,
        args.attestor,
        &args.key_id,
        "POST",
        path,
        now,
        body.as_bytes(),
    );
    println!(
        "curl -sS -X POST '{}{path}' -H 'content-type: application/json' -H 'kippu-signature: {signature}' -d '{body}'",
        args.base_url
    );
    Ok(())
}
