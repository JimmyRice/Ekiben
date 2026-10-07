//! Base45 ([RFC 9285](https://www.rfc-editor.org/rfc/rfc9285)), the text form of a ticket.
//!
//! Base45 uses exactly the 45 characters of a QR code's alphanumeric mode, which packs them at
//! 5.5 bits each. Two bytes become three characters, so a Base45 QR code is about as dense as a
//! binary one — but survives scanner SDKs that only hand back strings.

use crate::pinpon::Defect;

const ALPHABET: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

/// The number of bytes `text_len` Base45 characters decode to, or `None` if no valid Base45
/// text has that length.
pub const fn decoded_len(text_len: usize) -> Option<usize> {
    let pairs = text_len / 3;
    match text_len % 3 {
        0 => pairs.checked_mul(2),
        2 => match pairs.checked_mul(2) {
            Some(len) => len.checked_add(1),
            None => None,
        },
        _ => None,
    }
}

/// Decodes Base45 `text` into `out`, returning the filled prefix of `out`.
pub fn decode_into<'b>(text: &[u8], out: &'b mut [u8]) -> Result<&'b [u8], Defect> {
    let len = decoded_len(text.len()).ok_or(Defect::InvalidBase45)?;
    let out = out.get_mut(..len).ok_or(Defect::BufferTooSmall)?;

    let (groups, tail) = text.as_chunks::<3>();
    let (pairs, last) = out.as_chunks_mut::<2>();
    for (&[c, d, e], pair) in groups.iter().zip(pairs) {
        let value = combine(digit(c)?, digit(d)?, digit(e)?);
        *pair = u16::try_from(value)
            .map_err(|_| Defect::InvalidBase45)?
            .to_be_bytes();
    }
    // `decoded_len` guarantees a two-character tail exactly when there is a lone last byte.
    if let (&[c, d], [last]) = (tail, last) {
        let value = combine(digit(c)?, digit(d)?, 0);
        *last = u8::try_from(value).map_err(|_| Defect::InvalidBase45)?;
    }
    Ok(out)
}

/// Encodes `bytes` as Base45.
#[cfg(feature = "alloc")]
pub fn encode(bytes: &[u8]) -> alloc::string::String {
    let mut text = alloc::string::String::with_capacity(bytes.len().saturating_mul(2));
    for chunk in bytes.chunks(2) {
        let (value, digits) = match *chunk {
            [high, low] => (u32::from(u16::from_be_bytes([high, low])), 3),
            [last] => (u32::from(last), 2),
            _ => continue,
        };
        let mut rest = value;
        for _ in 0..digits {
            let (quotient, remainder) = (rest / 45, rest % 45);
            text.push(char::from(
                ALPHABET.get(remainder as usize).copied().unwrap_or(b'0'),
            ));
            rest = quotient;
        }
    }
    text
}

fn digit(character: u8) -> Result<u32, Defect> {
    let position = ALPHABET
        .iter()
        .position(|&candidate| candidate == character)
        .ok_or(Defect::InvalidBase45)?;
    u32::try_from(position).map_err(|_| Defect::InvalidBase45)
}

#[expect(
    clippy::arithmetic_side_effects,
    reason = "each digit is below 45, so the result is at most 91124"
)]
const fn combine(c: u32, d: u32, e: u32) -> u32 {
    c + d * 45 + e * 45 * 45
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;

    #[test]
    fn rfc_9285_examples() {
        for (bytes, text) in [
            (&b"AB"[..], "BB8"),
            (b"Hello!!", "%69 VD92EX0"),
            (b"base-45", "UJCLQE7W581"),
            (b"ietf!", "QED8WEX0"),
        ] {
            assert_eq!(encode(bytes), text);
            let mut buffer = [0; 16];
            assert_eq!(decode_into(text.as_bytes(), &mut buffer).unwrap(), bytes);
        }
    }

    #[test]
    fn rejects_invalid_text() {
        let mut buffer = [0; 16];
        assert_eq!(decode_into(b"GGW", &mut buffer), Err(Defect::InvalidBase45)); // 65536+
        assert_eq!(
            decode_into(b"ZZZZ", &mut buffer),
            Err(Defect::InvalidBase45)
        ); // length % 3 == 1
        assert_eq!(decode_into(b"ab", &mut buffer), Err(Defect::InvalidBase45)); // lowercase
        assert_eq!(
            decode_into(b"BB8", &mut [0; 1]),
            Err(Defect::BufferTooSmall)
        );
    }
}
