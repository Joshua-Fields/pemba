# Security policy

## Report a vulnerability privately

Use [GitHub private vulnerability reporting](https://github.com/Joshua-Fields/pemba/security/advisories/new)
for suspected vulnerabilities in pemba. Sign in to GitHub and use **Report a
vulnerability** in the repository's Security tab. The repository owner,
[@Joshua-Fields](https://github.com/Joshua-Fields), is the initial security maintainer.

Do not put exploit details, credentials, tokens, private schemas, relationship data
or other secrets in public issues, discussions or pull requests. Use synthetic data
and remove secrets from reproductions. If the private reporting option is unavailable,
do not post the report publicly; wait for the maintainer to restore the private route.
No alternative email address or public issue route is designated for security reports.

Include the affected version, Rust/Python and database versions, platform, relevant
configuration, expected versus actual authorization result, impact and a minimal safe
reproduction when possible. Distinguish an incorrect allow/deny from an evaluation
error. Report suspected authorization bypasses, fail-open error handling, limit bypasses,
transaction/isolation failures and dependency vulnerabilities through this route.

## Supported versions

The latest stable **0.1.x** release receives security and correctness maintenance on
a **best-effort** basis. Older versions receive case-by-case consideration. Before
the first stable release, available alpha releases and development builds receive
case-by-case consideration; this policy does not claim that stable 0.1.0 has shipped.
There is no response-time SLA, guaranteed patch or long-term support commitment.

The supported products are the Rust crates/CLI and Python package within their
documented scope. The browser playground uses WASM only for its own build; it does
not provide a supported WASM distribution or API contract.

## Maintainer handling

The repository owner reviews reports privately, assesses affected versions and impact,
and coordinates reproduction, remediation and disclosure with the reporter through
the private advisory. Keep exploit details and sensitive data in that private channel
while remediation is being coordinated. Fixes require regression coverage, including
fail-closed behavior and agreement with the independent evaluator where applicable.
Public advisories and release notes should describe the affected versions and remedy
without exposing credentials or private application data. Handling is best effort;
this process does not promise a response or disclosure deadline.

Normal bugs and feature requests without sensitive security details belong in
[GitHub Issues](https://github.com/Joshua-Fields/pemba/issues).

## Release safeguards and human gates

Security reporting activation and verification are human responsibilities. A policy
file alone does not prove that the route is enabled or usable. The release review in
[the 0.1.0 evidence index](docs/RELEASE-0.1.0.md#t78-security-reporting-and-release-safeguards)
records observed settings, checks, findings and outstanding human verification.

Publishing remains a human action after exact-candidate verification and approval.
Normal PR runs must not publish packages or receive publishing credentials. The
protected `pypi` environment requires human review before trusted publishing on a
release tag. Crates.io publication, tags, release approval and launch activation remain
the separate M7 human gates; this policy grants none of those approvals.
