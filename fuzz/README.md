# Fuzzing

Targets: `docs/07-testing.md` §5. Design choices: `docs/OPEN_QUESTIONS.md` OQ-039.

```bash
cargo install cargo-fuzz
cargo +nightly fuzz build
cargo +nightly fuzz run fuzz_schema_parse -- -max_total_time=600
```

| Target | Input | Must hold |
|---|---|---|
| `fuzz_schema_parse` | bytes as UTF-8 schema source | `compile` never panics |
| `fuzz_tuple_parse` | bytes as a relationship string | parsing never panics; parsed values round-trip |
| `fuzz_check_vs_reference` | bytes seeding the property-1 generators | engine `check` and `check_bulk` agree with the reference evaluator |

Seed corpora live in `corpus/<target>/` and are rebuilt by `python fuzz/seed_corpus.py`.
Commit the inputs libFuzzer adds during a run (§5). Crashes and hangs land in
`artifacts/` (ignored); reproduce one with `cargo +nightly fuzz run <target> <file>`.

**Windows:** cargo-fuzz builds with AddressSanitizer, and the targets need MSVC's
`clang_rt.asan_dynamic-x86_64.dll` on `PATH`, or they exit with `STATUS_DLL_NOT_FOUND`
(0xc0000135). It ships with the MSVC build tools under
`VC\Tools\MSVC\<version>\bin\Hostx64\x64`. `scripts/overnight.ps1` finds it with `vswhere`.
