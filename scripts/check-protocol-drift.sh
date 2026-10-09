#!/usr/bin/env bash
# check-protocol-drift.sh — protocol drift gate (GAP MAP v2, Part 5).
#
# Two modes:
#   static (default): verifies that the pinned values in
#     scripts/protocol-pins.json EXACTLY match the constants compiled into the
#     crates. Any disagreement means either the pins or the code drifted and the
#     gate fails. No network needed — safe for every CI run.
#   live (PROTOCOL_DRIFT_LIVE=1): additionally queries the chains and proves the
#     pinned programs/contracts actually exist and are executable. Writes
#     evidence/live/protocol_drift_check.json with status PASSED only when every
#     on-chain probe succeeded; otherwise FAILED. Never claims PASSED without
#     the probes having run.
#
# Exit 0 = no drift detected; exit 1 = drift (static or live) detected.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PINS="$ROOT/scripts/protocol-pins.json"
LIVE="${PROTOCOL_DRIFT_LIVE:-0}"
RPC_URL="${RPC_URL:-}"
EVIDENCE_DIR="${EVIDENCE_DIR:-$ROOT/evidence/live}"

if [ ! -f "$PINS" ]; then
  echo "check-protocol-drift: FAIL — pins file missing: $PINS" >&2
  exit 1
fi

STATIC_RESULT="$(ROOT="$ROOT" PINS="$PINS" python3 - <<'PY'
import json, os, re, sys
root = os.environ["ROOT"]
pins = json.load(open(os.environ["PINS"]))
errors = []

def read(p):
    return open(os.path.join(root, p), encoding="utf-8").read()

consts = read("crates/solana-kit/src/consts.rs")

# Map pin key -> (constant name in consts.rs)
sol_programs = {
    "system_program": "SYSTEM_PROGRAM",
    "token_program": "TOKEN_PROGRAM",
    "token_2022_program": "TOKEN_2022_PROGRAM",
    "associated_token_program": "ASSOCIATED_TOKEN_PROGRAM",
    "rent_sysvar": "RENT_SYSVAR",
    "compute_budget_program": "COMPUTE_BUDGET_PROGRAM",
    "pump_fun": "PUMP_PROGRAM_ID",
    "pump_fun_fees": "PUMP_FEES_PROGRAM_ID",
    "pumpswap_amm": "PUMPSWAP_PROGRAM_ID",
    "raydium_amm_v4": "RAYDIUM_AMM_V4",
    "raydium_clmm": "RAYDIUM_CLMM",
    "raydium_cpmm": "RAYDIUM_CPMM",
    "meteora_dlmm": "METEORA_DLMM",
    "meteora_damm_v1": "METEORA_DAMM_V1",
}
for key, const in sol_programs.items():
    m = re.search(r'static\s+' + const + r'\b.*?pk\("([^"]+)"\)', consts, re.S)
    if not m:
        errors.append(f"solana.programs.{key}: constant {const} not found in consts.rs")
        continue
    if m.group(1) != pins["solana"]["programs"][key]:
        errors.append(f"solana.programs.{key}: pin={pins['solana']['programs'][key]} code={m.group(1)}")

sol_mints = {"wsol": "WSOL_MINT", "usdc": "USDC_MINT"}
for key, const in sol_mints.items():
    m = re.search(r'static\s+' + const + r'\b.*?pk\("([^"]+)"\)', consts, re.S)
    if not m:
        errors.append(f"solana.mints.{key}: constant {const} not found")
        continue
    if m.group(1) != pins["solana"]["mints"][key]:
        errors.append(f"solana.mints.{key}: pin={pins['solana']['mints'][key]} code={m.group(1)}")

pump_accts = {
    "global": "PUMP_GLOBAL",
    "event_authority": "PUMP_EVENT_AUTHORITY",
    "fee_recipient_fallback": "PUMP_FEE_RECIPIENT_FALLBACK",
}
for key, const in pump_accts.items():
    m = re.search(r'static\s+' + const + r'\b.*?pk\("([^"]+)"\)', consts, re.S)
    if not m:
        errors.append(f"solana.pump_fun_accounts.{key}: constant {const} not found")
        continue
    if m.group(1) != pins["solana"]["pump_fun_accounts"][key]:
        errors.append(f"solana.pump_fun_accounts.{key}: pin={pins['solana']['pump_fun_accounts'][key]} code={m.group(1)}")

disc_names = {
    "initialize": "PUMP_DISC_INITIALIZE", "create": "PUMP_DISC_CREATE",
    "create_v2": "PUMP_DISC_CREATE_V2", "buy": "PUMP_DISC_BUY",
    "sell": "PUMP_DISC_SELL", "buy_exact_sol_in": "PUMP_DISC_BUY_EXACT_SOL_IN",
    "buy_v2": "PUMP_DISC_BUY_V2", "sell_v2": "PUMP_DISC_SELL_V2",
}
for key, const in disc_names.items():
    m = re.search(r'const\s+' + const + r'\s*:\s*\[u8;\s*8\]\s*=\s*\[([^\]]+)\]', consts)
    if not m:
        errors.append(f"solana.pump_fun_discriminators.{key}: constant {const} not found")
        continue
    code_vals = [int(x.strip()) for x in m.group(1).split(",")]
    if code_vals != pins["solana"]["pump_fun_discriminators"][key]:
        errors.append(f"solana.pump_fun_discriminators.{key}: pin={pins['solana']['pump_fun_discriminators'][key]} code={code_vals}")

if pins["solana"]["jupiter"]["quote_url"] not in consts:
    errors.append("solana.jupiter.quote_url: JUPITER_QUOTE_URL value not found in consts.rs")
if pins["solana"]["jupiter"]["swap_url"] not in consts:
    errors.append("solana.jupiter.swap_url: JUPITER_SWAP_URL value not found in consts.rs")

# Polymarket pins.
ctf = read("crates/module-polymarket/src/ctf.rs")
coll = read("crates/module-polymarket/src/collateral.rs")
v3 = read("crates/module-polymarket/src/exchange_v3.rs")
auth = read("crates/module-polymarket/src/auth.rs")
eip = read("crates/module-polymarket/src/eip712.rs")
cfg = read("crates/core/src/config.rs")

def grab(text, const):
    m = re.search(r'const\s+' + const + r'\s*(?::\s*&str)?\s*=\s*"([^"]+)"', text)
    return m.group(1) if m else None

checks = [
    (ctf, "CTF_ADDRESS_POLYGON", pins["polymarket"]["polygon"]["ctf_erc1155"], "polymarket.polygon.ctf_erc1155"),
    (coll, "COLLATERAL_ADDRESS_POLYGON", pins["polymarket"]["polygon"]["collateral_usdc_proxy"], "polymarket.polygon.collateral_usdc_proxy"),
    (v3, "V3_EXCHANGE_ADDRESS", pins["polymarket"]["polygon"]["ctf_exchange_v3"], "polymarket.polygon.ctf_exchange_v3"),
    (auth, "CLOB_AUTH_DOMAIN_VERSION", pins["polymarket"]["eip712"]["clob_auth_domain_version"], "polymarket.eip712.clob_auth_domain_version"),
    (eip, "V2_DOMAIN_VERSION", pins["polymarket"]["eip712"]["order_domain_version_v2"], "polymarket.eip712.order_domain_version_v2"),
    (v3, "V3_DOMAIN_VERSION", pins["polymarket"]["eip712"]["order_domain_version_v3"], "polymarket.eip712.order_domain_version_v3"),
]
for text, const, pin, label in checks:
    val = grab(text, const)
    if val is None:
        errors.append(f"{label}: constant {const} not found")
    elif val != pin:
        errors.append(f"{label}: pin={pin} code={val}")

for key, pin in pins["polymarket"]["api_hosts"].items():
    if pin not in cfg:
        errors.append(f"polymarket.api_hosts.{key}: value {pin} not found in config.rs defaults")

if errors:
    print("STATIC_DRIFT")
    for e in errors:
        print("  " + e)
else:
    print("STATIC_OK")
PY
)"

echo "$STATIC_RESULT"
case "$STATIC_RESULT" in
  STATIC_OK*) : ;;
  *)
    echo "check-protocol-drift: FAIL — static pin/code drift detected" >&2
    exit 1 ;;
esac

if [ "$LIVE" != "1" ]; then
  echo "check-protocol-drift: OK — static pins match the code (set PROTOCOL_DRIFT_LIVE=1 + RPC_URL for on-chain verification)."
  exit 0
fi

# ---------------------------------------------------------------------------
# Live mode: prove the pinned programs exist on-chain and are executable.
# ---------------------------------------------------------------------------
mkdir -p "$EVIDENCE_DIR"
LIVE_RESULT="$(ROOT="$ROOT" PINS="$PINS" RPC_URL="$RPC_URL" EVIDENCE_DIR="$EVIDENCE_DIR" python3 - <<'PY'
import json, os, sys, urllib.request, datetime

root = os.environ["ROOT"]
pins = json.load(open(os.environ["PINS"]))
rpc = os.environ.get("RPC_URL", "")
evidence_dir = os.environ["EVIDENCE_DIR"]

if not rpc:
    print("LIVE_SKIP: RPC_URL not set — refusing to claim on-chain verification")
    sys.exit(0)

def rpc_call(method, params):
    req = urllib.request.Request(
        rpc, data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
        headers={"content-type": "application/json"})
    with urllib.request.urlopen(req, timeout=20) as resp:
        return json.load(resp)

failures = []
probed = {}
upgradeable_loader = "BPFLoaderUpgradeab1e11111111111111111111111"
# Programs deliberately deployed under the immutable (non-upgradeable) loader.
# Verified on mainnet 2026-10-08: the SPL Associated Token program is owned by
# BPFLoader2111... and cannot be upgraded. Any other program must use the
# upgradeable loader. Keep this list minimal and evidence-backed.
expected_loader_override = {
    "associated_token_program": "BPFLoader2111111111111111111111111111111111",
}

for name, program_id in pins["solana"]["programs"].items():
    if name in ("system_program", "rent_sysvar", "compute_budget_program"):
        continue  # native programs are not owned by the upgradeable loader
    expected_loader = expected_loader_override.get(name, upgradeable_loader)
    try:
        res = rpc_call("getAccountInfo", [program_id, {"encoding": "base64"}])
        value = (res.get("result") or {}).get("value")
        if not value:
            failures.append(f"{name}: account {program_id} not found on chain")
            probed[name] = {"exists": False}
            continue
        owner = value.get("owner", "")
        executable = bool(value.get("executable"))
        probed[name] = {"exists": True, "executable": executable, "owner": owner}
        if not executable:
            failures.append(f"{name}: account {program_id} is not executable")
        elif owner != expected_loader:
            failures.append(f"{name}: account {program_id} owned by {owner}, expected {expected_loader}")
    except Exception as exc:  # noqa: BLE001 — report, don't crash the gate
        failures.append(f"{name}: RPC probe failed ({exc})")
        probed[name] = {"error": str(exc)}

status = "PASSED" if not failures else "FAILED"
evidence = {
    "validation_id": "protocol_drift_check",
    "provider": "solana_rpc",
    "environment": "live",
    "timestamp": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "status": status,
    "mode": "live",
    # Probe-based: verification lives in redacted_metadata.probed (on-chain
    # account facts), not a single external id, so it opts into self-attestation.
    "self_attesting": True,
    "endpoint_ref": "RPC_URL=<redacted>",
    "redacted_metadata": {"probed": probed, "failures": failures, "pins_version": pins["version"]},
}
out = os.path.join(evidence_dir, "protocol_drift_check.json")
json.dump(evidence, open(out, "w"), indent=2)
print(f"LIVE_{status}")
for f in failures:
    print("  " + f)
PY
)"

echo "$LIVE_RESULT"
case "$LIVE_RESULT" in
  LIVE_PASSED)
    echo "check-protocol-drift: OK — pins match code AND on-chain probes passed (evidence/live/protocol_drift_check.json)."
    exit 0 ;;
  LIVE_SKIP*)
    echo "check-protocol-drift: SKIPPED live verification ($LIVE_RESULT). Static check passed."
    exit 0 ;;
  *)
    echo "check-protocol-drift: FAIL — on-chain drift or probe failure detected" >&2
    exit 1 ;;
esac
