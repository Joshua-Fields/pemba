# 0.1.0 release evidence

This is the evidence index required by M7. No stable release is approved or published.
Only T7.2 is implemented here; T7.3–T7.17 remain separate tasks and gates.

## Preceding gates

| Requirement | Source | Completed task / evidence | Revision / result |
|---|---|---|---|
| Ordered M7 plan | T7.0 | [PR #80](https://github.com/Joshua-Fields/pemba/pull/80), merged | main `fee1cac`; merged 2026-10-29 |
| Maintainer scope and publication decisions | T7.1; OQ-044, OQ-058–OQ-060 | [PR #81](https://github.com/Joshua-Fields/pemba/pull/81), merged | main `8852eba`; merged 2026-10-29 |
| Previous implementation PR merged | M7 ordering | [PR #78](https://github.com/Joshua-Fields/pemba/pull/78), alpha.4 release preparation | merged; earlier release baseline only, not new verification evidence |

T7.1 approves sync Postgres scope, only Rust `check_str`, the extensible-type audit,
VitePress routes/content policy, and best-effort owner-led security/triage. Private
vulnerability reporting activation/verification remains human work at its later gates.
PR #79 is still open and unrelated to the T7.2 dependency gate; reassess it in T7.6/T7.11.

## T7.2 / T3.9 public API audit

Source: T3.9 all AC; T7.2 all AC; `06` §1–§2; `09` §3; OQ-044.
Implementation: this T7.2 PR on `t7.2-public-api-audit`; exact source commit and results
will be recorded after verification. [PUBLIC_API_AUDIT.md](PUBLIC_API_AUDIT.md) inventories
98 public structs/enums in all nine publishable crates, including facade/Postgres coverage.
Requests remain constructible downstream; custom stores retain producer constructors.
`check_str` and `Error::Parse` implement the approved helper; Python preserves ValueError.
Migration is in `06` and CHANGELOG. No storage format or package version changes.
OQ-061 records the conservative parse-code mapping clarification for review.

Verification is pending. No command, service-backed suite, platform CI job or skipped
check is claimed as passed by this initial entry. Release sign-off is not inferred.
