//! `kippu verify`: check a ticket offline with Kaisatsu, the way a gate does.

use std::io::Read;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
use kaisatsu::wire::{MAGIC, MAX_TICKET_LEN};
use kaisatsu::{TrustedKey, VerifiedTicket, Verifier};
use kippu_core::keys::parse_verifying_key;
use kippu_core::{Clock, SystemClock};
use kippu_domain::Timestamp;
use kippu_store::BoxError;

use crate::cli::{TicketEncoding, VerifyArgs};

#[expect(
    clippy::print_stdout,
    reason = "printing the verdict is the command's output"
)]
pub(crate) fn run(args: &VerifyArgs) -> Result<(), BoxError> {
    let keys = args
        .keys
        .iter()
        .enumerate()
        .map(|(index, key)| trusted_key(index, key))
        .collect::<Result<Vec<_>, _>>()?;
    let ticket = decode(&read_input(&args.ticket)?, args.encoding)?;
    let gate = Verifier::new(keys.as_slice());
    let verified = gate.verify(&ticket)?;
    print!("{}", describe(&verified));

    if let Some(issuer) = &args.issuer
        && verified.issuer() != issuer
    {
        return Err(format!("issued by {:?}, not {issuer:?}", verified.issuer()).into());
    }
    let at = args.at.unwrap_or_else(|| SystemClock.now());
    verified.check_time(u64::try_from(at.unix_seconds()).unwrap_or(0))?;
    println!("result       admitted at {at}");
    Ok(())
}

/// A key given on the command line, or the file holding it.
fn trusted_key(index: usize, key: &str) -> Result<TrustedKey, BoxError> {
    let text = if Path::new(key).is_file() {
        std::fs::read_to_string(key)?
    } else {
        key.to_owned()
    };
    let key = parse_verifying_key(&format!("--key #{}", index + 1), &text)?;
    Ok(TrustedKey::from_bytes(key.as_bytes())?)
}

/// The ticket argument: `-` for stdin, a file, or the text itself.
fn read_input(argument: &str) -> Result<Vec<u8>, BoxError> {
    let mut input = Vec::new();
    if argument == "-" {
        std::io::stdin().read_to_end(&mut input)?;
    } else if Path::new(argument).is_file() {
        input = std::fs::read(argument)?;
    } else {
        input = argument.as_bytes().to_vec();
    }
    Ok(input)
}

/// Decodes a ticket. `auto` takes the first decoding that yields a KP1 header.
fn decode(input: &[u8], encoding: TicketEncoding) -> Result<Vec<u8>, BoxError> {
    let text = || {
        std::str::from_utf8(input)
            .map(|text| text.trim_end_matches(['\r', '\n']))
            .map_err(|_| "the ticket is not text; use --encoding binary for raw bytes")
    };
    match encoding {
        TicketEncoding::Binary => Ok(input.to_vec()),
        TicketEncoding::Base64 => {
            let text = text()?.trim();
            [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
                .iter()
                .find_map(|engine| engine.decode(text).ok())
                .ok_or_else(|| "not valid base64".into())
        }
        TicketEncoding::Base45 => {
            let mut buffer = [0; MAX_TICKET_LEN];
            // Base45 has a space in its alphabet, so only line endings are trimmed.
            let ticket = kaisatsu::base45::decode_into(text()?.as_bytes(), &mut buffer)
                .map_err(|_| "not valid Base45 (or longer than a ticket can be)")?;
            Ok(ticket.to_vec())
        }
        TicketEncoding::Hex => Ok(hex::decode(text()?.trim())?),
        TicketEncoding::Auto => [
            TicketEncoding::Binary,
            TicketEncoding::Hex,
            TicketEncoding::Base64,
            TicketEncoding::Base45,
        ]
        .into_iter()
        .filter_map(|encoding| decode(input, encoding).ok())
        .find(|ticket| ticket.starts_with(&MAGIC))
        .ok_or_else(|| {
            "not a KP1 ticket in base64, Base45, hex or raw bytes; \
             pass --encoding to see why decoding fails"
                .into()
        }),
    }
}

/// The ticket's claims, one per line.
fn describe(ticket: &VerifiedTicket<'_>) -> String {
    let time =
        |seconds: u64| Timestamp::from_unix_seconds(i64::try_from(seconds).unwrap_or(i64::MAX));
    let mut lines = vec![
        format!("signature    valid, key {}", ticket.key_id()),
        format!("issuer       {}", ticket.issuer()),
        format!("ticket       {}", ticket.ticket_id()),
        format!("event        {}", ticket.event_id()),
        format!("ticket type  {}", ticket.ticket_type_id()),
        format!(
            "valid        {} until {}",
            time(ticket.valid_from()),
            time(ticket.valid_until())
        ),
        format!("issued       {}", time(ticket.issued_at())),
    ];
    for extension in ticket.extensions() {
        let value = match std::str::from_utf8(extension.value) {
            Ok(text) if !text.chars().any(char::is_control) => format!("{text:?}"),
            _ => format!("0x{}", hex::encode(extension.value)),
        };
        lines.push(format!("extension    {:#04x} = {value}", extension.tag));
    }
    lines.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use kaisatsu::{Claims, Issuer, Uuid};

    use super::*;

    fn ticket() -> (Issuer, Vec<u8>) {
        let issuer = Issuer::from_secret_key(&[7; 32]);
        let claims = Claims {
            issuer: "kippu.test".to_owned(),
            event_id: Uuid::from_bytes([1; 16]),
            ticket_id: Uuid::from_bytes([2; 16]),
            ticket_type_id: Uuid::from_bytes([3; 16]),
            valid_from: 1_000,
            valid_until: 2_000,
            issued_at: 500,
            extensions: [(0x80, b"VIP".to_vec()), (0x81, vec![0, 1])].into(),
        };
        let ticket = issuer.issue(&claims).unwrap();
        (issuer, ticket)
    }

    #[test]
    fn every_encoding_is_recognized() {
        let (_, ticket) = ticket();
        let encodings = [
            ticket.clone(),
            hex::encode(&ticket).into_bytes(),
            format!("{}\n", STANDARD.encode(&ticket)).into_bytes(),
            URL_SAFE_NO_PAD.encode(&ticket).into_bytes(),
            kaisatsu::base45::encode(&ticket).into_bytes(),
        ];
        for input in encodings {
            assert_eq!(decode(&input, TicketEncoding::Auto).unwrap(), ticket);
        }
        assert!(decode(b"hello", TicketEncoding::Auto).is_err());
    }

    #[test]
    fn verifies_against_the_given_key() {
        let (issuer, ticket) = ticket();
        let args = |key: String, at: &str| VerifyArgs {
            ticket: STANDARD.encode(&ticket),
            keys: vec![key],
            issuer: Some("kippu.test".to_owned()),
            encoding: TicketEncoding::Auto,
            at: Some(at.parse().unwrap()),
        };
        let key = STANDARD.encode(issuer.public_key());
        run(&args(key.clone(), "1970-01-01T00:20:00Z")).unwrap();
        let expired = run(&args(key, "1970-01-01T01:00:00Z")).unwrap_err();
        assert!(expired.to_string().contains("Expired"), "{expired}");

        let other = STANDARD.encode(Issuer::from_secret_key(&[8; 32]).public_key());
        let unknown = run(&args(other, "1970-01-01T00:20:00Z")).unwrap_err();
        assert!(unknown.to_string().contains("UnknownKey"), "{unknown}");
    }

    #[test]
    fn describes_extensions_as_text_or_hex() {
        let (issuer, ticket) = ticket();
        let key = TrustedKey::from_bytes(&issuer.public_key()).unwrap();
        let verified = Verifier::new(key).verify(&ticket).unwrap();
        let description = describe(&verified);
        assert!(description.contains("0x80 = \"VIP\""), "{description}");
        assert!(description.contains("0x81 = 0x0001"), "{description}");
        assert!(
            description.contains("valid        1970-01-01T00:16:40Z until"),
            "{description}"
        );
    }
}
