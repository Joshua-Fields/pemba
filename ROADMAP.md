# Public roadmap

This is direction, not a delivery schedule. There are no promised dates. Candidate
work needs its own specification, acceptance criteria and maintainer scope decision.
See [the milestones](docs/08-milestones.md) for the implementation plan,
[triage](docs/TRIAGE.md) for reports and [release evidence](docs/RELEASE-0.1.0.md)
for verification and human gates. Nothing here expands the supported scope.

## Shipped alphas and the stable target

The latest shipped release is **0.1.0-alpha.4**; **stable 0.1.0 is not yet published
or approved**. See [release history](CHANGELOG.md) and
[published releases](https://github.com/Joshua-Fields/pemba/releases).

| Release | Delivered capabilities |
|---|---|
| alpha.1 | Embedded Rust authorization engine and Python bindings; schema DSL, checks/lookups, memory/SQLite stores and caller-owned SQLite transactions |
| alpha.2 | Explain traces and EngineConfig setters |
| alpha.3 | Synchronous Postgres store and foreign writes, PendingRevision |
| alpha.4 | Browser playground and removal of deprecated Python integer conversions for pending revisions |
| Stable 0.1.0 target (unreleased) | Existing Rust/CLI/Python scope, sync SQLite/Postgres transactions, audited extensible APIs, guides and release verification; post-alpha.4 constructors and Rust check_str are development changes until released |

The [development guides](https://joshua-fields.github.io/pemba/docs/next/) may describe
APIs absent from alpha.4. Use a release's own documentation when running that version.
The playground uses an in-memory store; it cannot demonstrate a real database commit
or rollback and is not a supported WASM package.

## M7 remaining gates

T7.0–T7.11 are merged: scope decisions, API audit/documentation, user and transaction
guides, documentation site, contributor onboarding, security review, issue/roadmap
materials, launch drafts and the release rehearsal. Launch text remains unapproved.

- T7.12–T7.13: prepare stable metadata/migrations and verify the exact stable candidate.
- T7.14: human signs off T3.7 and the exact candidate and merges the release PR.
- T7.15: human tags and publishes through the reviewed publishing sequence.
- T7.16: read-only verification of public packages, documentation and playground.
- T7.17: human approves/posts launch materials and verifies/activates private reporting,
  labels, triage ownership and roadmap links. Reporting usability and authenticated
  registry ownership/access/publisher safeguards still require human confirmation
  before release sign-off/publication, as recorded in the evidence index.

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
