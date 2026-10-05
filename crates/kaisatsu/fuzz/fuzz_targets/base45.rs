//! Base45 decoding must never panic, and must round-trip whatever it accepts.
#![no_main]

use kaisatsu::base45;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &[u8]| {
    let mut buffer = [0; 4096];
    if let Ok(bytes) = base45::decode_into(text, &mut buffer) {
        assert_eq!(base45::encode(bytes).as_bytes(), text);
    }
});
