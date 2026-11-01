# Public roadmap

This is direction, not a delivery schedule. There are no promised dates. Candidate
work needs its own specification, acceptance criteria and maintainer scope decision.
See [the milestones](docs/08-milestones.md) for the implementation plan,
[triage](docs/TRIAGE.md) for reports and [release evidence](docs/RELEASE-0.1.0.md)
for verification and human gates. Nothing here expands the supported scope.

## Published releases

The latest shipped release is **stable 0.1.0**, published from commit
`41075c399f942b62280edeca81c0f68eaf314dab`. Public packages, documentation and the
playground passed T7.16 verification in [PR #93](https://github.com/Joshua-Fields/pemba/pull/93).
See [release history](CHANGELOG.md) and
[published releases](https://github.com/Joshua-Fields/pemba/releases).

| Release | Delivered capabilities |
|---|---|
| alpha.1 | Embedded Rust authorization engine and Python bindings; schema DSL, checks/lookups, memory/SQLite stores and caller-owned SQLite transactions |
| alpha.2 | Explain traces and EngineConfig setters |
| alpha.3 | Synchronous Postgres store and foreign writes, PendingRevision |
| alpha.4 | Browser playground and removal of deprecated Python integer conversions for pending revisions |
| Stable 0.1.0 | Rust/CLI/Python scope, sync SQLite/Postgres transactions, audited extensible APIs and constructors, Rust check_str, guides and public release verification |

The [development guides](https://joshua-fields.github.io/pemba/docs/next/) may describe
changes beyond stable 0.1.0. Use a release's own documentation when running that version.
The playground uses an in-memory store; it cannot demonstrate a real database commit
or rollback and is not a supported WASM package.

## M7 remaining gates

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

## After stable: scoped candidates

**T5.13 async foreign adapters follow stable**, as approved in T7.1. The shipped sync
Postgres scope is sufficient for 0.1.0; no raw async connection support is promised
today. This follow-up does not start automatically after launch.

| Milestone | Candidate scope and required constraints |
|---|---|
| M8 — Authorised queries | Filter application queries inside the database via permission predicates or a lookup_resources join, SQLite first; agree with the reference evaluator and keep pagination/ordering correct at a fixed revision |
| M9 — SpiceDB import/export | Schema/relationship conversion and migration guides in both directions; reject unsupported features, round-trip through differential tests and write original code from format documentation |
| M10 — User-driven features | Select from issues and launch feedback; detailed tasks are not planned in advance |

The existing top post-release performance follow-up is
[fewer Postgres round trips on deep hierarchies](docs/08-milestones.md#m10--user-driven-features)
([OQ-056](docs/OPEN_QUESTIONS.md#oq-056--postgres-latency-is-round-trips-two-per-statement),
[measured context](benches/RESULTS.md#postgres-rust-linux)). Batched reads per level or
a recursive query are candidates, not a promised implementation. Any optimization
must preserve one snapshot and agree with the independent evaluator.

Other M10 candidates include a FastAPI helper, Node bindings and a supported WASM
build. Items excluded by [CLAUDE §6](CLAUDE.md#6-things-that-are-explicitly-out-of-scope-until-told-otherwise)
need an explicit human scope decision first. No caveats/CEL, expiration, Watch,
server mode, distributed dispatch or SpiceDB wire compatibility is approved here.

## Known differences and limits

Read [known SpiceDB differences](crates/pemba-difftest/KNOWN_DIFFERENCES.md),
[bounds and troubleshooting](docs/guides/troubleshooting.md) and
[transaction constraints](docs/guides/transactions.md). Errors fail closed as errors,
distinct from denial. Historical reads use the current schema and are subject to GC;
foreign writes need caller commit and do not promise reads of uncommitted data.
Postgres schema rename recovery remains unimplemented (OQ-062); do not rename an
active schema. Report safe normal bugs/features through
[Issues](https://github.com/Joshua-Fields/pemba/issues); suspected vulnerabilities use
[SECURITY.md](SECURITY.md) privately. Support remains best effort with no response SLA.
