# 0.3.0 release record

**0.3.0 is merged to `main`. It is not tagged or published.** PR #102 merged `841b5f1` as
`73b08d2` on 2026-11-02 at 22:58:47 UTC. This document supplies no approval, and the merge is an
observation rather than a recorded approval. Published 0.2.0 and its [record](RELEASE-0.2.0.md) remain unchanged.
When 0.3.0 is published, add a publication record here, as 0.2.0 did, and keep the text below
as evidence.

## Decisions for the maintainer

A ticked box records a decision the maintainer stated. Neither this record nor green CI makes
any of them.

- [x] **OQ-066: the T10.1 performance results. Accepted by the maintainer on 2026-11-02.** No
  scenario regressed consistently across three runs. The 5% limit on each run was below this
  workflow's measured noise (unchanged write code swung +9.6% to -9.0% at p50), so the strict
  reading is not treated as a failure. `wide_groups_10k_below_head` (+30.5 / -20.6 / -3.2, with its
  baseline alone spanning 1,343 to 1,927 µs) is accepted as noise and not investigated now; watch it
  in future benchmark runs. Details are in OQ-066 and
  [`benches/RESULTS.md`](../benches/RESULTS.md#postgres-before-and-after-three-runs-oq-066).
- [x] **The 409 for a bound ahead of the store.** Accepted by the maintainer on 2026-11-02, with no
  `Retry-After` (OQ-067).
- [x] **The release date.** `CHANGELOG.md` has `## [0.3.0] - 2026-11-03`, the day the maintainer
  plans to tag. The release notes are extracted from that section, so if the tag moves to another
  day, change the date first.
- [ ] **Explicit approval of the exact candidate SHA** before tag and publication.

Until publication, `main` names 0.3.0 in `README.md` and the guides and lists it under releases
in `ROADMAP.md`. That text points to the releases page for what is published.

Still open, for the maintainer alone:

- [ ] Protected `pypi` environment review of the tag artifacts.
- [ ] Authenticated registry ownership, publisher, access and 2FA settings.
- [ ] Private security report form and its notifications.
- [ ] Announcement decision, and M7/T7.17 community activation.

## Scope and compatibility

- **Fewer Postgres round trips (T10.1, OQ-056, OQ-065, OQ-066).** Read-ahead batches and
  generic plans inside a read snapshot. `folders_depth_30` 99 → 33 round trips and
  `lookup_resources_10k` 506 → 8.
- **FastAPI helper (T10.2, OQ-067).** `pemba.contrib.fastapi.Authz` and the extra
  `pemba[fastapi]`, specified in `06` §2.4. Pure Python, and `import pemba` does not need it.
- **Public docs cleanup.** `ROADMAP.md` became visitor-facing, and process state moved to
  `MAINTAINER-STATUS.md`.
- **No migration from 0.2.0.** Storage formats are unchanged. `ReadStore::batch_read_limit` and
  `ReadStore::read_batch` are provided methods, so a custom store keeps compiling.
- Rust and Python packages share 0.3.0. `Cargo.lock` and `fuzz/Cargo.lock` change only pemba's
  own versions: 23 version lines, nothing else.
- CPython >=3.11 is retained: the [official version-status table](https://devguide.python.org/versions/)
  listed 3.11 with status `security` when checked on 2026-11-02, as ADR-014 requires.
- M8, announcement and community activation are not part of this release.

## Verification record

Local checks ran on 2026-11-02 against `e710a46b0819b8e19a1ae0b11d1c645cad0faa32`, on Windows 11
x86_64, with Rust 1.99.0, Python 3.12.10 and Node from the system install. The commit that adds
this record changes only documentation; CI on the release PR covers its exact SHA.

### Observed locally on Windows

| Check | Result |
|---|---|
| `cargo fmt --check`, `clippy -D warnings`, rustdoc with `-D warnings`, fuzz fmt and check | Pass |
| `cargo test --workspace --all-features`, without `PEMBA_PG_URL` | 705 passed, 21 ignored |
| The same with `--ignored` | 21 passed |
| Python suite, tools only, no FastAPI | 175 passed, 4 skipped |
| `mypy --strict tests`, no FastAPI | Pass, 19 files |
| Python suite with FastAPI 0.162.0 and httpx 0.28.1 (`ci.yml` pins), without Postgres | 392 passed, 47 skipped |
| `mypy --strict tests python/pemba/contrib`, same pins | Pass, 21 files |
| FastAPI tests at the floor, FastAPI 0.115.3 and httpx 0.27.2, without Postgres | 220 passed, 45 skipped |
| `maturin build --release`, `maturin sdist`, `ci/check_contrib_packaged.py` | Pass |
| Wheel in a clean venv, no extra: version, `site-packages` path, engine calls, `ImportError` names the extra | Pass |
| Wheel with `[fastapi]`: small app, `def` and `async def` routes, 200, 403, 422, 401, 409 | Pass |
| Sdist built and installed away from the checkout, same two checks | Pass |
| `cargo publish --workspace --dry-run --locked`, nine crates, no upload | Pass |
| `playground/conformance/run.sh` | Native and WASM transcripts identical, 45,323 lines |
| Playground: `npm ci`, `audit --omit=dev` (0 vulnerabilities), `wasm`, 173 unit tests, `build`, `size` (585 KB of 700 KB), 72 e2e tests in Chromium | Pass |
| Docs site: 10 script tests, `npm run build` for `next` at 0.3.0 | Pass |
| `release.mjs notes 0.3.0` extracts the section, `prerelease 0.3.0` is `false` | Pass |

### Observed locally on Postgres 17.11, Windows

Two throwaway clusters under `target/` on private ports, one plain and one with TLS and an
untrusted self-signed certificate. They were removed afterwards.

| Check | Result |
|---|---|
| `cargo test -p pemba-store-postgres --all-features` | 74 passed, 2 ignored |
| `--test revision_order -- --ignored` | Pass |
| `--no-default-features` (plaintext-only build) | 74 passed |
| `ci/check_tls.py` | Pass |
| All-driver Python tests with `PEMBA_PG_REQUIRED` and `PEMBA_FASTAPI_REQUIRED` | 369 passed, 8 skipped |
| `mypy --strict tests`, same venv | Pass |

The 8 skips are by construction: the memory store cannot fail underneath the engine, a lock
test needs a real store lock, and only Postgres has an unresolved `PendingRevision`.

### Only remote CI can prove

These were not observed locally:

- Linux and macOS builds and tests, and the platform wheels and their smoke matrix.
- Python 3.11 (`python-oldest`, with SQLite 3.26.0). Locally only 3.12 was available.
- Postgres 15 and 18. Locally only 17 was available.
- `cargo deny` and the other CI-only jobs.

Observed on PR #102 at `841b5f1`: 42 checks passed, including all of the above except macOS Python,
and 4 publishing and deployment jobs were skipped. **`python (macos-latest)` never ran there:** it
was cancelled after 15 minutes with "The job was not acquired by Runner of type hosted even after
multiple attempts", with no step started and no rerun, and the PR merged with it in that state.
**Differential tests against SpiceDB passed on PR #102**, on memory and on Postgres, so the earlier
text here saying they had not run on this candidate was wrong.

Observed on `main` at `73b08d2`: CI and Playground passed, including `python (macos-latest)`. That
is the first run of the macOS Python suite and the FastAPI tests on this code.

Full property tests last passed on their schedule at `5cd51bf`, the T10.1 merge, and did not run on
this candidate; the 1,000-case property job in CI did. Since `5cd51bf` the only change under any
`crates/*/src` is the version in a doc comment in `crates/pemba/src/lib.rs`.

### Not run

- Registry installs, docs.rs, release assets, live documentation and playground routes: they
  cannot exist before publication.

### CI before this candidate

CI and Playground passed on `f7aa60b`, the merge of PR #101. CI failed on `5cd51bf`: the
`postgres 15` service container stopped during init (`FATAL: the database system is shutting
down`), before any test ran. It did not recur on `f7aa60b`.

## Human review, publication and recovery

Follow `09-repo-and-tooling.md` §4.1. After the required checks are green, the maintainer
reviews the exact candidate SHA, the decisions above, the release text and the API scope.

1. Merge the pre-tag PR. As a docs-only PR it skips the code jobs, so check that CI passes on the
   resulting `main` SHA, including `python (macos-latest)`: pushes to `main` run everything.
2. Record explicit approval of that `main` SHA.
3. Tag `v0.3.0` on approved `main`. `wheels.yml` builds and tests the tag's artifacts. Review
   the protected `pypi` environment, then let trusted publishing upload them. Do not substitute
   local rebuilds.
4. `cargo publish` the nine crates, waiting for each registry version before a dependent. The
   established order still satisfies the 0.3.0 manifests, including versioned development
   dependencies: schema → core → sqlite-common → memory → sqlite → postgres → fixtures → facade
   → CLI. Private, test, Python-only and WASM crates are not published on crates.io.
5. Check Pages: the GitHub Release has `playground-0.3.0.tar.gz`, the deploy is green, and both
   `https://joshua-fields.github.io/pemba/` and `…/pemba/v/0.3.0/` load with 0.3.0 in their
   footers.
6. Independently verify registry installs, including `pemba[fastapi]`, docs.rs, release assets,
   docs and playground routes before any announcement.

Never replace an existing tag, registry version, release asset or public artifact. On partial
publication, stop later uploads, inventory immutable source and digests, and retry only a
confirmed missing artifact with unchanged approved source, in order. A packaging or source
defect needs a separately reviewed corrected version and new verification and approval.
