# Support & handover model

What handover concretely consists of, category by category. This document
promises no SLA, no support duration, and no availability commitment — those
are contractual matters between the parties, not properties of the software.
What follows are the factual transfer categories and the artifacts that make
each one self-serviceable.

## 1. Source handover

- **Artifact:** the git repository at freeze commit `0e139c3` (on release
  commit `9c677cd`), 146 tracked files, clean tree, no secrets, no build
  artifacts (`.gitignore`/`.dockerignore` enforced; secret-scan gate passed).
- **Buyer verification:** `docs/BUYER-DEPLOYMENT.md` §1–2 (commit, file
  count, version identity).
- **Completeness rule:** everything needed to build, test and run is in the
  tree — proven by the from-zero rebuild on a wiped machine
  (`docs/HANDOVER.md` §2).

## 2. Deployment handover

- **Artifacts:** `Dockerfile`, `docker-compose.yml`, `.env.template`,
  `config.toml.example` (annotated reference for every key),
  `docs/DEPLOYMENT.md`, `docs/BUYER-DEPLOYMENT.md` (15-step sequence).
- **Boundary:** the seller delivers the procedure and templates; the buyer
  executes them on buyer infrastructure. Docker image build/smoke and CI were
  NOT EXECUTED in the delivery environment — the buyer's first successful run
  of each is their verification of those paths.

## 3. Configuration handover

- **Artifacts:** `config.toml.example` (all sections annotated),
  README §Configuration (precedence: defaults → TOML → .env → env overrides;
  unknown keys rejected), `docs/OPERATIONS.md`.
- **Safety property:** the defaults are safe — paper mode, all modules
  disabled, strict config parsing (typos fail startup instead of silently
  changing behavior), `[signing] provider` values that are not implemented
  fail startup rather than falling back.

## 4. Incident handover

- **Artifacts:** `docs/OPERATIONS.md` runbook (alerts, degradation matrix,
  emergency stop, journal inspection, audit verification), kill-switch
  surfaces (`POST /api/kill`, Telegram `/kill`, cross-replica flag sync),
  `docs/RECONCILIATION.md` (ambiguity matrix), `docs/BACKUP-RESTORE.md`
  (Redis-loss and DB-restore behavior).
- **Expectation:** incident response after transfer is staffed by the buyer
  using these runbooks; nothing in the software phones home or depends on the
  seller at runtime.

## 5. Security contact handover

- **State at delivery:** root `SECURITY.md` deliberately points at "the
  security contact of the current repository owner" — no real address ships,
  because publishing a fake one would misroute vulnerability reports.
- **Action:** at transfer, the receiving party publishes their own contact
  in `SECURITY.md` (tracked item: `docs/HANDOVER.md` §5.4,
  `docs/ACCEPTANCE-CHECKLIST.md`).

## 6. Credential rotation

At transfer, **every credential that ever existed on either side should be
considered compromised-by-default and rotated**, regardless of the secret-scan
evidence (which is clean). Checklist:

- Solana keypair(s) — generate new keys for production; never reuse keys that
  existed in any test environment.
- Polymarket/Polygon private key — new key, new API credentials (L1/L2
  headers derive from it).
- Telegram bot token — revoke & reissue via BotFather; update allow-lists.
- `API_KEY` — new value; it gates all mutating routes.
- PostgreSQL / Redis credentials — new; PG is reachable only from the compose
  network by default, but rotate anyway.
- RPC/Geyser/PumpPortal provider API keys — re-contract in the buyer's name.

The software stores secrets only in the environment, so rotation requires no
code change — restart with new env values.

## 7. Ownership transfer

- **Legal:** copyright holder insertion in `LICENSE` (currently the
  placeholder "sniper-suite authors"); any IP assignment paperwork is between
  the parties (`docs/SCOPE-BOUNDARY.md` §4).
- **Technical:** repository ownership/remote transfer; set the real
  `repository` URL in the workspace `Cargo.toml` at publish time.
- **What ownership does NOT include:** third-party protocols, APIs, brands or
  services (itemized in `docs/IP-COMPONENTS.md` §Summary and
  `docs/THIRD-PARTY.md` §3).

## 8. Repository transfer

- Transfer mechanism is buyer/seller-agreed (git bundle, hosted-repo
  transfer, or fresh private remote). Verify on receipt: commit hashes
  `9c677cd` + `0e139c3` present, `git status` clean, `release-check.sh`
  green on the buyer's machine (`docs/BUYER-DEPLOYMENT.md` §9).
- History note: the repository carries the full commit history through the
  freeze; `AUDIT.md` preserves the per-pass engineering evidence independent
  of git.

## 9. Staking program ownership

- **State at delivery:** source + host tests + PREVIOUSLY VERIFIED BPF build
  and validator e2e. **Not deployed to any cluster**; `declare_id!` is a
  pre-deploy placeholder.
- **Transfer actions (buyer):** deploy under the placeholder id with the
  matching keypair (or change id + keypair and rebuild); initialize with the
  admin set to a **multisig PDA** (Squads/Realms recommended — the program
  deliberately does not embed M-of-N logic); set production timelock ≥ 24h;
  plan the one-shot `GenesisMint`; commission the external audit **before**
  mainnet (`docs/STAKING.md`, README §"Deploying the staking program").
- Whoever holds the admin key(s) controls governance within the program's
  caps and timelock — custody of that multisig is the real ownership
  question, and it is a buyer-side responsibility from deployment onward.

## 10. Production sign-off

A factual definition of "done" for the transfer, matching
`docs/ACCEPTANCE-CHECKLIST.md`:

1. Full local gate (`release-check.sh`) green on buyer infrastructure.
2. Paper-mode run stable on buyer infra; health/readiness/metrics verified;
   Telegram verified live; crash-recovery drill passed; backup→restore drill
   passed on the buyer's PG.
3. (If simulating) `simulate` mode exercised — real transactions built and
   RPC-simulated, nothing broadcast.
4. (If trading live) both live gates deliberately enabled, risk limits sized,
   gradual funded validation under operator supervision.
5. (If deploying staking) external audit passed, multisig admin in place,
   program id finalized, genesis plan executed once.
6. Legal fill-ins complete: LICENSE holder, repository URL, security contact.

Until every applicable line is checked, the system is "delivered" but not
"signed off" — the distinction matters and is intentional.
