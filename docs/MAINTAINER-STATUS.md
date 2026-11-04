# Maintainer status

Internal release and process state. The visitor-facing page is
[`ROADMAP.md`](../ROADMAP.md); keep task IDs, gates and approval language here or in
[`08-milestones.md`](08-milestones.md), not there.

## Current state (2026-11-03)

- **Latest published release: 0.3.0**, tagged on `7d0b3c6` and published 2026-11-03: public docs
  cleanup; fewer Postgres round trips on deep hierarchies (T10.1, OQ-056); FastAPI helper (T10.2,
  `06` §2.4). Tag, GitHub Release, registry versions and post-publication checks are recorded in
  [`RELEASE-0.3.0.md`](RELEASE-0.3.0.md#publication-record), with the human-only items still open.
  Announcement remains a human step.
- **0.4.0 is a candidate, not merged, tagged or published:** the certificate pin (T10.5),
  `Engine::close` (T10.3) and delete-in-transaction (T10.4), which is on `main`.
  Open decisions are in [`RELEASE-0.4.0.md`](RELEASE-0.4.0.md).
- **OQ-066 is accepted** (2026-11-02): no consistent regression across three benchmark runs. Watch
  `wide_groups_10k_below_head` in future runs.
- **M7 remains open.** T7.17 (announcement, private reporting, labels, triage ownership) is
  human work. Neither a merge nor a publication establishes it.
- **M8 authorised queries** is the intended headline of a later release, with no version set.
  Nothing is implemented, and its task breakdown is drafted only on request.
- **M9 SpiceDB import/export is deprioritised:** SpiceDB service users are unlikely to
  migrate to an embedded library.

## Release safeguards

Moved from `SECURITY.md`, whose public text now only summarises this.

Security reporting activation and verification are human responsibilities. A policy
file alone does not prove that the route is enabled or usable. The release review in
[the 0.1.0 evidence index](RELEASE-0.1.0.md#t78-security-reporting-and-release-safeguards)
records observed settings, checks, findings and outstanding human verification.

Publishing remains a human action after exact-candidate verification and approval.
Normal PR runs must not publish packages or receive publishing credentials. The
protected `pypi` environment requires human review before trusted publishing on a
release tag. Crates.io publication, tags, release approval and launch activation remain
the separate M7 human gates; this policy grants none of those approvals.

## Archive: earlier roadmap text

Moved from `ROADMAP.md` when that page became visitor-facing. It is preserved as evidence
and reads as of `7d890758c391c1ace82ae34b766bcde7a200c787`, the `v0.2.0` tag; only relative
links were adjusted. Statements about 0.2.0 being unreleased were superseded when it was
published. Use "Current state" above for what holds now.

### Published releases

The latest shipped release is **stable 0.1.0**, published from commit
`41075c399f942b62280edeca81c0f68eaf314dab`. Public packages, documentation and the
playground passed T7.16 verification in [PR #93](https://github.com/Joshua-Fields/pemba/pull/93).
See [release history](../CHANGELOG.md) and
[published releases](https://github.com/Joshua-Fields/pemba/releases).

The [development guides](https://joshua-fields.github.io/pemba/docs/next/) may describe
changes beyond stable 0.1.0. Use a release's own documentation when running that version.
The playground uses an in-memory store; it cannot demonstrate a real database commit
or rollback and is not a supported WASM package.

### M7 remaining gates

T7.0–T7.11 are merged: scope decisions, API audit/documentation, user and transaction
guides, documentation site, contributor onboarding, security review, issue/roadmap
materials, launch drafts and the release rehearsal. Launch text remains unapproved.

- T7.12–T7.13: stable metadata, migrations and exact-candidate verification completed in
  [PR #92](https://github.com/Joshua-Fields/pemba/pull/92), now merged.
- T7.14–T7.15: release-PR merge, tag and publication are observed. Those observations
  do not establish explicit human sign-off or authenticated registry/account and
  reporting confirmations; retain/confirm the human records identified in the evidence index.
- T7.16: completed public verification of all nine Rust crates, Python wheels/sdist,
  GitHub Release, documentation and playground; evidence is in merged PR #93.
- T7.17: remains human work. The maintainer chooses announcement text, destinations
  and timing, posts approved materials and verifies/activates private reporting,
  labels, triage ownership and roadmap links. No announcement or community activation
  completion is inferred. **M7 remains open** until its human acceptance criteria are met.

Green CI alone is not release approval. This roadmap authorizes no merge, tag,
publication, deployment or announcement.

### After stable: scoped candidates

**T5.13 async foreign mode is complete on main**, merged in
[PR #95](https://github.com/Joshua-Fields/pemba/pull/95) after passing CI. It adds
`write_async` for psycopg, asyncpg and SQLAlchemy async transactions, plus `pemba.aio`
read helpers. See [implementation and verification evidence](T5.13-ASYNC.md).
The maintainer approved the resolve helper and cancellation handoff in OQ-064.

These additions are **unreleased**: published stable 0.1.0 retains synchronous
Postgres foreign writes. The maintainer selected **0.2.0** for the next release;
[candidate preparation and verification](RELEASE-0.2.0.md) are in progress.
This is not release approval or publication. The bridge benchmark establishes no performance threshold requiring
a sans-IO redesign; that remains a separately scoped candidate. M8 and the later
candidates below have not started.

| Milestone | Candidate scope and required constraints |
|---|---|
| M8 — Authorised queries | Filter application queries inside the database via permission predicates or a lookup_resources join, SQLite first; agree with the reference evaluator and keep pagination/ordering correct at a fixed revision |
| M9 — SpiceDB import/export | Schema/relationship conversion and migration guides in both directions; reject unsupported features, round-trip through differential tests and write original code from format documentation |
| M10 — User-driven features | Select from issues and launch feedback; detailed tasks are not planned in advance |

The existing top post-release performance follow-up is
[fewer Postgres round trips on deep hierarchies](08-milestones.md#m10--user-driven-features)
([OQ-056](OPEN_QUESTIONS.md#oq-056--postgres-latency-is-round-trips-two-per-statement),
[measured context](../benches/RESULTS.md#postgres-rust-linux)). Batched reads per level or
a recursive query are candidates, not a promised implementation. Any optimization
must preserve one snapshot and agree with the independent evaluator.

The FastAPI helper is T10.2 (OQ-067), the last item of the 0.3.0 scope. Other M10
candidates include Node bindings and a supported WASM build. Items excluded by [CLAUDE §6](../CLAUDE.md#6-things-that-are-explicitly-out-of-scope-until-told-otherwise)
need an explicit human scope decision first. No caveats/CEL, expiration, Watch,
server mode, distributed dispatch or SpiceDB wire compatibility is approved here.

### Known differences and limits

Read [known SpiceDB differences](../crates/pemba-difftest/KNOWN_DIFFERENCES.md),
[bounds and troubleshooting](guides/troubleshooting.md) and
[transaction constraints](guides/transactions.md). Errors fail closed as errors,
distinct from denial. Historical reads use the current schema and are subject to GC;
foreign writes need caller commit and do not promise reads of uncommitted data.
Postgres schema rename recovery remains unimplemented (OQ-062); do not rename an
active schema. Report safe normal bugs/features through
[Issues](https://github.com/Joshua-Fields/pemba/issues); suspected vulnerabilities use
[SECURITY.md](../SECURITY.md) privately. Support remains best effort with no response SLA.
