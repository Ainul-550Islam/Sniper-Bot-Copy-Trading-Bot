#!/usr/bin/env bash
# Apply the repository's single tracked-junk cleanup owner before staging.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
bash scripts/cleanup-tracked-junk.sh --apply
