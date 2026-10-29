# Issue triage

Normal bugs and feature requests belong in [GitHub Issues](https://github.com/Joshua-Fields/pemba/issues),
using the bug or feature form. Search existing reports and the [roadmap](../ROADMAP.md)
first. Suspected vulnerabilities, including authorization bypasses, use the private
route in [SECURITY.md](../SECURITY.md). Never request exploit details, credentials,
production data or private share links in a public thread. If sensitive material
appears publicly, direct the reporter to the private route without repeating it;
the owner handles removal and private follow-up.

## Ownership and commitments

The repository owner, [@Joshua-Fields](https://github.com/Joshua-Fields), is the initial
issue-triage maintainer (OQ-060). Contributors can help reproduce and propose fixtures;
the owner decides disposition and scope. Maintenance of the latest stable 0.1.x is
best effort; older releases receive case-by-case consideration. Before stable exists,
alpha/development reports receive case-by-case consideration. There is no response-time
SLA, guaranteed patch or long-term support commitment. An issue, priority note or
roadmap candidate promises neither implementation nor a delivery date.

This document prepares the process. **T7.17 requires the human maintainer to verify
labels and ownership and activate triage.** No label setup or new response commitment
is authorized here. Any change to the approved best-effort policy needs a human
decision recorded in the release evidence.

## Existing label vocabulary

Use the repository's existing labels below; this task creates no labels or automatic
labelling rules. Maintainer verification/setup remains T7.17. Severity and priority
are written in the triage comment, not invented label names.

| Label | Use |
|---|---|
| `bug` | Reproducible behavior contrary to the specification |
| `enhancement` | New capability or behavior change requiring scope review |
| `documentation` | Incorrect, missing or unclear documentation |
| `accessibility` | Barriers affecting people with disabilities |
| `question` | Clarification or more reproduction information needed |
| `duplicate` | Same underlying problem as a linked canonical report |
| `invalid` | Reproduction establishes that the report does not describe a project defect; explain why |
| `wontfix` | Maintainer declines work with a reason; not a silent dismissal |
| `good first issue` | Maintainer has established a small, well-specified contribution |
| `help wanted` | Maintainer welcomes contribution on agreed scope |
| `dependencies` | Dependency update or dependency-specific maintenance |
| `javascript` | JavaScript/tooling changes, including existing dependency automation |

## Severity and priority

Severity describes observed impact; priority describes the maintainer's next action.
Record the affected versions, store/platform and evidence, rather than inferring
severity from the reporter's label. Suspected security impact moves to private reporting.

| Severity | Meaning |
|---|---|
| Critical | Suspected incorrect authorization grant, fail-open behavior or transaction/isolation failure with security impact: handle privately under SECURITY |
| High | Confirmed non-sensitive correctness failure, data loss or unusable supported path without a practical workaround |
| Moderate | Supported behavior fails but has a safe workaround |
| Low | Documentation, usability or diagnostic issue with limited impact |

| Priority | Meaning, without a response deadline |
|---|---|
| Release blocker | Violates a required release gate; resolve and rerun affected evidence before approval |
| Next scoped fix | Maintainer accepts a bounded correction for a separate task/PR |
| Candidate | Needs design, demand or scope decision; no scheduled implementation |

## Handling a report

1. Check for security sensitivity first. For a safe public report, check version,
   store/platform, minimal synthetic schema/tuples, query, expected/actual result and
   reproduction. Distinguish denial from an evaluation error. Request missing facts
   without requesting secrets. Installation-only reports may explain why a model is
   not applicable. Compare against [known differences](../crates/pemba-difftest/KNOWN_DIFFERENCES.md)
   and [limits/troubleshooting](guides/troubleshooting.md).
2. Search for the same root cause. Link a duplicate to the canonical issue, preserve
   useful reproduction/version differences there and close with an explanation.
   Avoid copying sensitive content. A related symptom alone does not prove duplication.
3. Reproduce on the reported version, then current development where practical, using
   disposable stores. Record commands, versions and actual outcomes, including tests
   that could not run. Apply existing labels and record severity/priority and owner.
4. For a confirmed regression, reduce it to a fixture in `testdata/fixtures/` or the
   appropriate storage, binding, WASM or tooling regression test. Watch it fail before
   fixing it. Evaluator changes must agree with the independent reference evaluator;
   storage bugs need the affected backend's service-backed checks. Follow
   [CONTRIBUTING](../CONTRIBUTING.md) and CLAUDE's focused/full-check iteration rule.
5. If behavior is unspecified or specifications disagree, record a genuine ambiguity
   in [OPEN_QUESTIONS](OPEN_QUESTIONS.md), use the conservative choice and flag it.
   Promote accepted work to a numbered task with references, acceptance criteria,
   dependencies and tests in [the milestones](08-milestones.md). API/format changes
   update the specification and migration notes together; accepted ADR or out-of-scope
   changes require the human decision first. A feature report alone grants no scope.
6. Link the fixture/test, task and PR back to the report. The owner closes after the
   fix is merged or explains another disposition. State the first containing release
   only when known: merged is not published. Track deferred work as roadmap candidates.
