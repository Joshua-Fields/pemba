//! `docs/07-testing.md` §5: arbitrary bytes → `Relationship::from_str` must never panic, and
//! on success the relationship round-trips through its string form.

#![no_main]

use libfuzzer_sys::fuzz_target;
use pemba_core::model::Relationship;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(rel) = s.parse::<Relationship>() {
        let printed: u32 = rel.to_string();
        let again: Relationship = printed.parse().unwrap_or_else(|e| {
            panic!("{s:?} parsed, but its printed form {printed:?} did not: {e}")
        });
        assert_eq!(rel, again, "{s:?} does not round-trip through {printed:?}");
    }
});
