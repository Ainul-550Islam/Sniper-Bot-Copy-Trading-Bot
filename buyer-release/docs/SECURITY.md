# Security policy

## Reporting a vulnerability

Do **not** open a public issue or pull request for security problems.

Report privately to the security contact of the current repository owner
(the contact is handed over together with this repository; see
`docs/HANDOVER.md` §"Handover fill-ins"). Include: reproduction steps,
affected version(s) (`VERSION`), impact, and any suggested mitigation.
Expect an initial response as soon as the maintaining team triages it;
there is no published SLA — this project has **no external audit and no
bug-bounty program** (see below).

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.1.0   | yes (initial handover release) |

## Scope and posture

The full threat model, secret-handling rules, signer boundary, RBAC model,
database/Redis security, RPC-provider trust assumptions, staking admin and
timelock limitations, and emergency procedures live in
**[docs/SECURITY.md](docs/SECURITY.md)**.

Key facts a reporter should know up front:

- The suite defaults to **paper** mode; live trading requires two explicit
  configuration gates plus real key material.
- Secrets are read from the environment (or an explicitly configured
  secrets file), never committed; the repository is scanned for secret
  material at every release (`scripts/release-check.sh`).
- The on-chain staking program has **NOT** been externally audited. It is
  provided as-is under the terms in `LICENSE`, and the documentation
  explicitly blocks mainnet deployment until an independent audit passes.

## What this document is not

No external security audit, penetration test, or formal verification has
been performed on any component of this repository. Nothing here or in
`docs/` claims otherwise.
