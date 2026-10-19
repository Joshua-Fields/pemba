//! `docs/07-testing.md` §5: arbitrary bytes → `compile` must never panic.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The schema API takes `&str`; invalid UTF-8 never reaches the lexer.
    if let Ok(src) = std::str::from_utf8(data) {
        let _ = warden_schema::compile(src);
    }
});
