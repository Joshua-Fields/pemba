# 0.2.0 release record

**0.2.0 is tagged and published.** Publication is recorded below. The sections after it
are the preparation text as written before publication, kept as evidence. This document
supplies no approval. Published stable 0.1.0 and its historical evidence remain unchanged.

## Publication record

Read from public sources on 2026-11-01. This is an observation of what is public, not a
record of who approved it.

**Git and GitHub.** Tag `v0.2.0` is a lightweight tag on
`7d890758c391c1ace82ae34b766bcde7a200c787`, the merge of release PR #97. The
[GitHub Release](https://github.com/Joshua-Fields/pemba/releases/tag/v0.2.0) was published
at 2026-11-01T12:42:11Z and is neither a draft nor a pre-release.

**crates.io** (`crates.io/api/v1/crates/<name>`). All nine shipping crates report 0.2.0 as
their newest version, none yanked. Upload order matches the order required below.

| Crate | Version | Uploaded (UTC) |
|---|---|---|
| `pemba-schema` | 0.2.0 | 2026-11-01T12:40:32Z |
| `pemba-core` | 0.2.0 | 2026-11-01T12:40:42Z |
| `pemba-sqlite-common` | 0.2.0 | 2026-11-01T12:40:50Z |
| `pemba-store-memory` | 0.2.0 | 2026-11-01T12:40:56Z |
| `pemba-store-sqlite` | 0.2.0 | 2026-11-01T12:41:03Z |
| `pemba-store-postgres` | 0.2.0 | 2026-11-01T12:41:12Z |
| `pemba-fixtures` | 0.2.0 | 2026-11-01T12:41:20Z |
| `pemba` | 0.2.0 | 2026-11-01T12:41:33Z |
| `pemba-cli` | 0.2.0 | 2026-11-01T12:41:44Z |

**PyPI** (`pypi.org/pypi/pemba/json`). `pemba` reports 0.2.0 as its latest version, with
none of these files yanked. They were uploaded from 2026-11-01T12:45:36Z to 12:45:44Z:

- `pemba-0.2.0-cp311-abi3-macosx_10_12_x86_64.whl`
- `pemba-0.2.0-cp311-abi3-macosx_11_0_arm64.whl`
- `pemba-0.2.0-cp311-abi3-manylinux_2_17_aarch64.manylinux2014_aarch64.whl`
- `pemba-0.2.0-cp311-abi3-manylinux_2_17_x86_64.manylinux2014_x86_64.whl`
- `pemba-0.2.0-cp311-abi3-win_amd64.whl`
- `pemba-0.2.0.tar.gz`

### Human items, not confirmed by this record

Public metadata cannot establish any of these. They remain for the maintainer to confirm:

- [ ] Explicit approval of the exact candidate SHA before merge, tag and publication.
- [ ] Protected `pypi` environment review of the tag artifacts.
- [ ] Authenticated registry ownership, publisher, access and 2FA settings.
- [ ] Private security report form and its notifications.
- [ ] Announcement decision, and M7/T7.17 community activation.

### Not checked by this record

The post-publication checks named at the end of this document were not run here: registry
installs, docs.rs, release assets, and the live documentation and playground routes.

## Preparation record

The maintainer explicitly selected 0.2.0 in this chat on 2026-11-01, authorizing
release preparation and verification only. Base main is
`406d8bfe2350c7dc41f94c401a5b22f01f13de28`, including merged T5.13 PR #95 and
status/decision PR #96. Follow `09-repo-and-tooling.md` §4.1, ADR-014/ADR-016,
OQ-055/OQ-057/OQ-064 and the corrected publication-order lesson in
[0.1.0 evidence](RELEASE-0.1.0.md#derived-cratesio-publication-order--human-only).

## Scope and compatibility

- New Python APIs: `Engine.write_async` for psycopg `AsyncConnection`, asyncpg,
  SQLAlchemy `AsyncSession`/`AsyncConnection`; `pemba.aio.check`, `lookup_resources`,
  `lookup_subjects` and `resolve(pending)`. OQ-064's resolve/cancellation behavior
  is explicitly approved. Caller-owned transactions and error propagation are retained.
- Synchronous Rust/Python APIs, storage formats, supported backend/SQLite floors,
  external dependency versions and runtime implementations are unchanged by release preparation.
  No migration is required from 0.1.0. Existing sync code keeps working; async users
  opt into the new entry points. Cancellation drains work, then the caller rolls back.
- Rust/Python packages share 0.2.0. Root/fuzz locks must change only own package versions;
  README copies, changelog extraction, stubs, typing files, licences and docs metadata
  must agree. Test-only/private/benchmark/playground crates remain unpublished on crates.io.
- CPython >=3.11 is retained: the [official version-status table](https://devguide.python.org/versions/)
  still lists 3.11 as supported (security maintenance through October 2027), checked
  2026-11-01 as required by ADR-014. Existing wheel smoke matrix covers 3.11 and 3.14;
  no unobserved interpreter/platform test is called a pass. WASM remains playground-only.
- No M8, sans-IO redesign, announcement or community activation is part of this release.
  M7/T7.17 and outstanding reporting/account/sign-off confirmations remain separate.

## Exact-candidate verification record

The canonical record will be the description of the release PR from branch
`release/0.2.0` targeting main. Before human approval it must identify the full
40-character candidate SHA, actual commands/workflow URLs, results and explicit
skipped/unavailable/failing checks. Pending work is not technical completion.
The preparation commit includes this index; verification results can be recorded
in the PR description without changing the frozen candidate. Any source change
requires affected checks and a new candidate identity.

Required candidate checks:

| Area | Required evidence |
|---|---|
| Local source | Full CLAUDE §5 suite once: workspace fmt/Clippy/tests/rustdoc, separate fuzz fmt/build, isolated tool-only Python build/tests/strict typing. Identify ignored slow cases separately. |
| Async/sync services | Required PG15/18 all-driver suites, app+relationship commit/rollback, pending resolution, cancellation and SQLSTATE/mismatch checks; untrusted TLS rejection. |
| Packages | Locked workspace publish dry run for all nine shipping crates; normalized dependencies/no local production paths; LICENSE/NOTICE/README and Python stub/typing/sdist contents. No upload. |
| Python artifacts | Build candidate wheel/sdist; rebuild sdist and install away from the source checkout; assert version/path and run async/sync API, transactions and typing smoke checks. Platform wheel/installed-service CI must pass before merge. |
| Playground | Fresh native/WASM conformance, wasm32 checks, npm clean install/audit/wasm/unit/build/size/browser checks per §4.1. No deployment. |
| Documentation/release | Regression tests and strict stable/next builds; 0.2.0 notes extraction and non-prerelease classification; historical 0.1.0 notes retained byte-for-byte. |
| Remote CI | Candidate PR CI, Wheels, Playground and applicable differential runs. Skipped publishing/deployment jobs remain skips. |

The detailed implementation and benchmark record is [T5.13-ASYNC.md](T5.13-ASYNC.md).
Its measurements remain historical; no new performance threshold or benchmark run is
invented for a metadata-only release candidate.

## Human review, publication and recovery

After technical verification and current required checks are green, the maintainer
reviews the exact candidate SHA, release text/API scope, limitations and security
findings. Confirm outstanding authenticated registry ownership/publisher/access/2FA
and private-report form/notification checks; successful earlier uploads do not prove
those confirmations. Record explicit approval and merge the reviewed release PR,
checking that main's intended tree matches the approved candidate.

Only then may the human tag `v0.2.0` on approved main. Review the protected PyPI
environment after its tag artifacts pass testing; publish those artifacts through
the existing trusted-publishing workflow. Do not substitute local rebuilds.

Derive the nine-crate publication order from candidate manifests, including packaged
versioned development dependencies. The established order to verify is schema → core
→ sqlite-common → memory → sqlite → postgres → fixtures → facade → CLI. Wait for each
registry version before publishing a dependent package. Private/test/Python-only and
WASM crates are not additional crates.io releases.

Never replace an existing tag, registry version, release asset or public artifact.
On partial publication, stop later uploads, inventory immutable source/digests and
retry only a confirmed missing artifact with unchanged approved source in order.
A packaging/source defect needs a separately reviewed corrected version and new
verification/approval. After publication, independently verify registry installs,
docs.rs, release artifacts, docs and playground routes before any human announcement.
