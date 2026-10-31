# sniper-suite — operator entry points (PART 03, K5).
#
# Every target is a thin wrapper over something that already exists in this
# repository: a script under scripts/ or tests/release/, an npm script of the
# control plane, or a cargo invocation CI itself runs. No target re-implements
# a check, and no target hides a failure — each runs the real command.
#
# `make help` only reads this file: it needs no cargo, no node_modules, no
# docker daemon and no network. `make -n <target>` prints the exact commands.

SHELL := /usr/bin/env bash
CARGO ?= cargo
COMPOSE ?= docker compose
NPM ?= npm
# The workspace member that owns main.rs (the server + all modules).
SUITE ?= sniper-suite
UI ?= apps/control-plane

.PHONY: help init up down migrate run test gates e2e clean frontend-deps test-services-up test-services test-services-down

help: ## List every target (no toolchain, docker or network required)
	@awk 'BEGIN {FS = ":.*?## "} /^[A-Za-z0-9_.-]+:.*?## / {printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2}' $(MAKEFILE_LIST)
	@echo ""
	@echo "  First run:      make init && make up && make migrate && make run"
	@echo "  Before commit:  make test        (fmt, clippy, workspace + UI suites)"
	@echo "  Before release: make gates       (every cargo-free repository gate)"
	@echo "  UI end to end:  make e2e         (Playwright, builds and serves the UI itself)"

init: ## Create config.toml and .env from the shipped examples (never overwrites)
	@./scripts/bootstrap-config.sh
	@echo "next: fill in the RPC endpoint and module switches in config.toml,"
	@echo "      POSTGRES_PASSWORD / REDIS_URL in .env, and the wallet through the"
	@echo "      environment (SOLANA_KEYPAIR) — never in a tracked file."

up: ## Build and start the full stack: bot + PostgreSQL + Redis + control plane
	@test -f config.toml || { echo "no ./config.toml — run 'make init' first (compose mounts it read-only)"; exit 1; }
	@test -f .env || { echo "no ./.env — run 'make init' first (compose reads POSTGRES_* from it)"; exit 1; }
	@$(COMPOSE) up -d --build
	@echo "stack up: API + dashboard on http://localhost:8080 (GET /ready for the probe)"

down: ## Stop the stack. Named volumes keep their data ('make clean' does not touch them)
	@$(COMPOSE) down

migrate: ## Apply the SQL migrations and exit (needs the database from `make up`, or DATABASE_URL)
	@MIGRATE_ONLY=1 $(CARGO) run -p $(SUITE) -- --migrate-only

run: ## Run the bot in PAPER mode against ./config.toml (never live, whatever config says)
	@EXECUTION_MODE=paper $(CARGO) run --release -p $(SUITE)

frontend-deps: ## Install the control-plane dependencies (npm ci) for the UI targets
	@$(NPM) --prefix $(UI) ci

test: ## Full test sweep: fmt, clippy -D warnings, workspace tests, UI typecheck/lint/unit
	@$(CARGO) fmt --all --check
	@$(CARGO) clippy --workspace --all-targets -- -D warnings
	@$(CARGO) test --workspace -- --test-threads=1
	@$(CARGO) test -p $(SUITE) --lib
	@test -d $(UI)/node_modules || { echo "no $(UI)/node_modules — run 'make frontend-deps' first (the UI targets need it)"; exit 1; }
	@$(NPM) --prefix $(UI) run typecheck
	@$(NPM) --prefix $(UI) run lint
	@$(NPM) --prefix $(UI) run test:unit

test-services-up: ## Start disposable PostgreSQL 16 and Redis 7 test services
	@$(COMPOSE) -f docker-compose.test.yml -p sniper-round1-test up -d --wait

test-services: ## Run all service-backed integration gates and tear down the test stack
	@COMPOSE="$(COMPOSE)" ./scripts/test-with-services.sh

test-services-down: ## Stop and remove the disposable test stack and its data
	@$(COMPOSE) -f docker-compose.test.yml -p sniper-round1-test down --volumes --remove-orphans

gates: ## Every cargo-free repository gate (hygiene, migrations, openapi, routes, secrets, contracts)
	@bash scripts/forbid-fake-data.sh
	@bash scripts/verify-migration-graph.sh
	@bash scripts/verify-script-modes.sh
	@bash scripts/verify-delivery.sh
	@bash scripts/check-openapi-coverage.sh
	@bash scripts/check-routes-vs-ui.sh
	@bash scripts/scan-secrets.sh
	@bash tests/release/helm_image_contract.sh
	@bash tests/release/marketing_claims.sh
	@echo "  note: tests/release/stats_current.sh and manifest_current.sh are the"
	@echo "        release step's job (they recheck docs/STATS.md and"
	@echo "        release-manifest.json after scripts/generate-stats.sh /"
	@echo "        update-release-manifest.sh run); tests/release/buyer_*.sh need the"
	@echo "        generated buyer-release/ tree, which is not in this repository."

e2e: ## Playwright end-to-end run of the control plane (it builds and serves the UI itself)
	@test -d $(UI)/node_modules || { echo "no $(UI)/node_modules — run 'make frontend-deps' first"; exit 1; }
	@$(NPM) --prefix $(UI) run test:e2e

clean: ## Remove the build artifacts this repository produces (target/ and UI output)
	@$(CARGO) clean
	@rm -rf $(UI)/.next $(UI)/playwright-report $(UI)/test-results
	@echo "build output removed; the compose stack, its volumes and config.toml were left alone — use 'make down' for the stack"
