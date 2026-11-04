# 0.4.0 release record

**0.4.0 is a release candidate. It is merged to `main` (#110, `dcb01f4`), but not tagged or
published.** This document supplies no approval. Published 0.3.0 and its [record](RELEASE-0.3.0.md) remain unchanged.
When 0.4.0 is published, add a publication record here, as 0.3.0 did, and keep the text below
as evidence.

## Decisions for the maintainer

Ticked items are the maintainer's decisions of 2026-11-04. Neither this record nor green CI makes
any of them.

- [x] **Delete-in-transaction is in this release.** Accepted by the maintainer. It reached `main`
  when #107 merged, and OQ-069 is accepted. Needle does not need it.
- [x] **What the next versions are called.** 0.5.0 is the Django helper (`pemba.contrib.django`,
  Tasks 3 and 4 of its plan). 0.6.0 is authorised queries (M8). `docs/08-milestones.md` says so.
- [x] **The release date.** The day the maintainer tags. `CHANGELOG.md` keeps
  `## [0.4.0] - YYYY-MM-DD` until the maintainer confirms the Railway test, and it must be filled
  before tagging. The release notes are extracted from that section.
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

- **In:** `pinned_certificate_pem` (T10.5, ADR-017, OQ-070), `Engine::close` (T10.3, OQ-068) and
  delete-in-transaction (T10.4, OQ-069).
- **Out:** the Django helper (0.5.0) and authorised queries (M8, 0.6.0).
- **Compatibility:** additive. `PostgresConfig` is `#[non_exhaustive]`, `WritableStore::close` is
  a provided method, and storage formats and migrations are unchanged from 0.3.0.
- **Behaviour without the new options:** unchanged, including verification against the platform
  trust store and `sslmode=prefer` continuing in plaintext when a server refuses TLS.

**The reason for this release is confirmed in production.** Needle's Railway service logged
`invalid peer certificate: Other(OtherError(CaUsedAsEndEntity))`, and the tests reproduce that
message on Railway's public `postgres-ssl` image. **A pinned connection has not been tested
against a real Railway database,** only against that image. Needle's first pinned deployment is
that test.

## Verification record

Run locally on Windows on 2026-11-04, on `fee47e2` (this candidate rebased on `main` at `a62cdcf`)
plus text-only changes to `CHANGELOG.md`, `ROADMAP.md`, `docs/MAINTAINER-STATUS.md` and this
record, against disposable Postgres 18 containers: plain, snakeoil and Railway's image by digest.

| Check | Result |
|---|---|
| `cargo fmt`, clippy, `cargo doc`, `cargo deny`, fuzz fmt and check | Pass |
| `cargo test --workspace --all-features` with services | Pass |
| `pemba-store-postgres` without TLS; `revision_order -- --ignored` | Pass |
| pytest with CI drivers and services | 594 passed, 8 skipped |
| pytest in a tool-only venv, no services | 206 passed, 4 skipped |
| `mypy --strict tests`, both venvs | Pass |
| `examples/django`, Postgres and SQLite | Pass |
| Docs site tests and build | Pass |
| `cargo publish --workspace --dry-run --locked` | Pass, nothing uploaded |
| `playground/conformance/run.sh` | Native and WASM transcripts identical (45,323 lines) |
| Playground `npm audit --omit=dev` | 0 vulnerabilities |
| Playground `wasm`, `test`, `build`, `size`, `e2e` | Pass; 174 unit tests; 585.4 KB of a 700 KB budget; 72 e2e passed |
| `release.mjs notes 0.4.0` and `prerelease 0.4.0` | Extracts the section; `false` |

### Python 3.14 smoke test

`wheels.yml` failed on Python 3.14, on all four platforms that ran it on an early commit of #109:
`test_cancelling_aio_close_still_closes[memory]` raced. On 3.14 the close finished before the
cancel in 232 of 300 runs, against 0 of 300 on 3.11, and the engine closed in all 600. The test
was wrong, not `close`. #109 fixed the test, and `wheels.yml` passed on #109 afterwards. PyPI
publishing waits for every smoke job, so check `wheels.yml` is green on this candidate before
tagging.

### Only remote CI can prove

- `wheels.yml` on this candidate's own `Cargo.lock`, on every platform.
- The Postgres job on 15 and 18, including Railway's image by digest.

### Not run

- A pinned connection to a real Railway database.
- The wider `--ignored` suite beyond `revision_order`.
