# Docker verification status: BLOCKED in vendor sandbox (no daemon)

The vendor's packaging/hardening sandbox had no Docker daemon, so the image
build, `docker compose config -q`, and the container smoke test were NOT
executed by the vendor. They are recorded as BLOCKED — never as PASS.

What WAS executed (native equivalent, labeled as such):
- The server binary was built from this exact source and started with
  `config.toml.example` against a restored PostgreSQL 17.11 + Redis 8.0.2;
  `/health`, `/ready`, `/api/health`, `/api/status` (paper mode,
  live_allowed=false), `/api/audit/verify`, `/metrics` all served; clean
  SIGTERM shutdown — `../evidence/app-startup/phase8b-*.log`.

Buyer action (step 19 of `docs/BUYER-ACCEPTANCE-TEST.md`):
`docker compose config -q && docker build -t sniper-suite:local . && docker
run … /api/health` — or let the CI `docker` job run it (step 20).
The Dockerfile is multi-stage, non-root, healthchecked, pinned to
`rust:1.98.1-bookworm`.
