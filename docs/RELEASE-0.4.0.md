# 0.4.0 release record

**0.4.0 is a release candidate. It is not merged, tagged or published.** This document
supplies no approval. Published 0.3.0 and its [record](RELEASE-0.3.0.md) remain unchanged.
When 0.4.0 is published, add a publication record here, as 0.3.0 did, and keep the text below
as evidence.

## Decisions for the maintainer

None of these has been made by this record or by green CI.

- [ ] **Confirm the cause in production. Unconfirmed, and blocking.** 0.4.0's reason to exist is
  Needle's failure to connect to its Railway database, which has only been reproduced locally.
  Needle's `authz_initialize_failed` log should show the real error. 0.4.0 targets the problem
  if it shows `TLS handshake` and `invalid peer certificate`, with `UnknownIssuer`,
  `CaUsedAsEndEntity` or `not valid for name`. On Railway's public image the expected reason is
  `CaUsedAsEndEntity`. Any other error means this release does not fix it.
- [ ] **Review and merge #109, then rebase this candidate on `main`.** ADR-017 and OQ-070 were
  confirmed in the session; the code has not had a maintainer's review.
- [ ] **GitGuardian on #109.** Incident 39585763 is the fake password of a disposable CI server,
  and an allowlist entry now covers it. The check was still failing after that, so mark the
  incident in the dashboard if this is the only remaining failure.
- [ ] **What the next versions are called.** `docs/oq-068-accepted` (#107) says "0.4.0 is the
  Django release" and M8 is 0.5.0, and its T10.4 says delete-in-transaction is 0.4.0. All three
  are now wrong, and #107 will conflict with this candidate. Here, M8 has no version.
- [ ] **The release date.** `CHANGELOG.md` has `## [0.4.0] - YYYY-MM-DD` for the maintainer to
  fill before tagging. The release notes are extracted from that section.
- [ ] **Explicit approval of the exact candidate SHA** before merge, tag and publication.
- [ ] **Text on `main` between merge and publication.** `README.md` and the guides name 0.4.0
  and point to the releases page for what is published, and `ROADMAP.md` lists 0.4.0 under
  releases. That text stays true after publication and is visible on `main` before it. Merge
  close to publication if this matters.

Still open, for the maintainer alone:

- [ ] Protected `pypi` environment review of the tag artifacts.
- [ ] Authenticated registry ownership, publisher, access and 2FA settings.
- [ ] Announcement decision.

## Scope and compatibility

- **In:** `pinned_certificate_pem` (T10.5, ADR-017, OQ-070) and `Engine::close` (T10.3, OQ-068).
- **Out:** delete-in-transaction (T10.4), which Needle does not need, authorised queries (M8) and
  the Django helper.
- **Compatibility:** additive. `PostgresConfig` is `#[non_exhaustive]`, `WritableStore::close` is
  a provided method, and storage formats and migrations are unchanged from 0.3.0.
- **Behaviour without the new options:** unchanged, including verification against the platform
  trust store and `sslmode=prefer` continuing in plaintext when a server refuses TLS.

**Not tested against a real Railway database.** The pin was tested against Railway's public
`postgres-ssl` image run locally, and that image can differ from what a service runs.

## Verification record

Run locally on Windows on 2026-11-04, on an earlier commit of this candidate (`14087ca`, before
the test fixes described here and a one-line fix to a new release-notes test), against disposable Postgres 18 containers: plain, snakeoil and Railway's image
by digest.

| Check | Result |
|---|---|
| `cargo fmt`, clippy, `cargo doc`, `cargo deny`, fuzz fmt and check | Pass |
| `cargo test --workspace --all-features` with services | Pass |
| `pemba-store-postgres` without TLS; `revision_order -- --ignored` | Pass |
| pytest with CI drivers and services | 552 passed, 8 skipped |
| pytest in a tool-only venv, no services | 197 passed, 4 skipped |
| `mypy --strict tests`, both venvs | Pass |
| `examples/django`, Postgres and SQLite | Pass |
| Docs site tests and build | Pass |
| `cargo publish --workspace --dry-run --locked` | Pass, nothing uploaded |
| `playground/conformance/run.sh` | Native and WASM transcripts identical (45,323 lines) |
| Playground `npm audit --omit=dev` | 0 vulnerabilities |
| Playground `wasm`, `test`, `build`, `size`, `e2e` | Pass; 585.4 KB of a 700 KB budget; 72 e2e passed |
| `release.mjs notes 0.4.0` and `prerelease 0.4.0` | Extracts the section; `false` |

### Python 3.14 smoke test

`wheels.yml` failed on Python 3.14, on all four platforms that ran it on #109:
`test_cancelling_aio_close_still_closes[memory]` raced. On 3.14 the close finished before the
cancel in 232 of 300 runs, against 0 of 300 on 3.11, and the engine closed in all 600. The test
was wrong, not `close`. #109 fixes the test, which passed 50 of 50 repeats on 3.14 and 3.11 with
CI's wheel from before the fix. PyPI publishing waits for every smoke job, so check `wheels.yml`
is green on this candidate before tagging.

### Only remote CI can prove

- `wheels.yml` on this candidate's own `Cargo.lock`, on every platform.
- The Postgres job on 15 and 18, including Railway's image by digest.

### Not run

- A real Railway database.
- The wider `--ignored` suite beyond `revision_order`.
