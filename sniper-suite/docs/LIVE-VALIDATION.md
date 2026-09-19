# Live-validation runbook (operator-controlled)

How to validate the LIVE execution paths against real networks — step by
step — without ever letting the system trade money that was not explicitly
staged for it. Every step below is a READ or a SIMULATION unless it is
explicitly marked **FUNDED STEP (operator-controlled)**.

## 0. Evidence taxonomy (labeling rules)

| Label | What it proves | What it does NOT prove |
|---|---|---|
| `paper` | App logic end-to-end with demo balances | Nothing about any network |
| `simulate` | Real RPC transport + node-side simulation verdicts | No transaction is ever broadcast; no funds move |
| `live read` | Real on-chain state reads (balances/decimals/allowances) | No order placement |
| `funded devnet canary` | Real broadcast + confirmation on devnet (valueless or dust) | Nothing about mainnet liquidity/behavior |
| `funded live` | Real money execution | Only provable by the operator, with staged funds, at their own risk |

A paper test is NOT live evidence. A simulation is NOT a funded execution. A
mocked RPC is NOT mainnet proof. Every artifact you keep from this runbook
must carry one of the labels above.

## 1. Polymarket (Polygon) — live-read validation, no orders

The app's live path (`module-polymarket`: `read_collateral` /
`ensure_live_funding`) refuses to size or send an order unless a fresh
(≤ `collateral_max_age_secs`, default 15 s) on-chain read succeeded. You can
run the SAME reads by hand with plain `curl` before ever enabling live mode.

Constants used below (verify them against Polymarket's current documentation
before use — do not trust any single source, including this file):

```bash
RPC="https://polygon-rpc.com"                 # your trusted Polygon RPC
PUSD="0xC011a7E12a19f7B1f670d46F03B03f3342E82DFB"   # collateral token (proxy)
CTF="0x4D97DCd97eC945f40cF65F87097ACe5EA0476045"    # ConditionalTokens (ERC-1155)
EXCH="0xE111180000d2663C0091e4f400237545B87B996B"   # CTF Exchange V2
NEG_RISK_EXCH="0xe2222d279d744050d28e00520010520000310F59" # Neg-risk CTF Exchange V2
WALLET="<your-20-byte-wallet-hex-no-0x>"
PAD="000000000000000000000000"
```

### 1.1 RPC reachability

```bash
curl -s "$RPC" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}'
# expected: {"jsonrpc":"2.0","id":1,"result":"0x89"}   (137 = Polygon mainnet)
```

### 1.2 Real collateral balance (`balanceOf`, selector `0x70a08231`)

```bash
curl -s "$RPC" -H 'Content-Type: application/json' -d '{
  "jsonrpc":"2.0","id":1,"method":"eth_call",
  "params":[{"to":"'"$PUSD"'","data":"0x70a08231'"$PAD$WALLET"'"},"latest"]}'
# result is a 32-byte hex uint256 in RAW units.
```

### 1.3 Decimals sanity (`decimals()`, selector `0x313ce567`)

```bash
curl -s "$RPC" -H 'Content-Type: application/json' -d '{
  "jsonrpc":"2.0","id":1,"method":"eth_call",
  "params":[{"to":"'"$PUSD"'","data":"0x313ce567"},"latest"]}'
# expected: 0x...06  → 6 decimals. The app rejects implausible values
# (outside 1..=18) as BalanceUnavailable — it never guesses a scale.
```

### 1.4 Exchange allowance (`allowance`, selector `0xdd62ed3e`)

Required ONLY for `signature_type = 0` (EOA-direct) orders; proxy flows
(1/2/3) hold funds inside the proxy and are balance-checked only. Check
against the exchange that will settle the market (neg-risk markets settle
via the neg-risk exchange):

```bash
curl -s "$RPC" -H 'Content-Type: application/json' -d '{
  "jsonrpc":"2.0","id":1,"method":"eth_call",
  "params":[{"to":"'"$PUSD"'","data":"0xdd62ed3e'"$PAD$WALLET$PAD${EXCH#0x}"'"},"latest"]}'
# uint256 raw allowance. The app requires allowance ≥ order notional for
# signature_type 0 and REJECTS the order (InsufficientFunding) otherwise —
# it never falls back to a paper balance in live mode.
```

### 1.5 Outcome-token holdings (ERC-1155 `balanceOf`, selector `0x00fdd58e`)

```bash
TOKEN_ID="<decimal token id from the CLOB API>"
# data = 0x00fdd58e + pad(wallet) + uint256(tokenId)
python3 - "$WALLET" "$TOKEN_ID" <<'PY'
import sys
w, t = sys.argv[1], int(sys.argv[2])
print("0x00fdd58e" + "0"*24 + w + hex(t)[2:].rjust(64, "0"))
PY
# feed the printed data into an eth_call against $CTF.
```

### 1.6 In-app verification (no orders)

1. Start in paper: `EXECUTION_MODE=paper CONFIG_PATH=./config.toml ./target/release/sniper-suite`
2. `curl -s localhost:8080/api/status` → `mode: paper`.
3. Stop. Set the real Polygon RPC + wallet in `config.toml`
   (`[polymarket]`), export the signer env per `.env.template`.
4. Start with `EXECUTION_MODE=simulate` → the module performs real CLOB/Gamma
   reads and node-side simulations but broadcasts nothing.
5. Only after 1.1–1.5 all match what the app reports (`/api/polymarket/*`
   state endpoints), stage a small dedicated amount and switch to `live`.
   The first live order attempt re-runs `ensure_live_funding` — with an
   unstaged wallet it must REJECT with `BalanceUnavailable`/
   `InsufficientFunding` before any broadcast. That rejection is itself
   part of the validation: a live system that orders without verified
   funding is the defect this gate exists to prevent.

**FUNDED STEP (operator-controlled):** placing a real order requires real
pUSD staged by the operator, a real key, and is executed solely at the
operator's discretion. Nothing in this repository stages funds or flips
`EXECUTION_MODE` automatically.

## 2. Solana — RPC, wallet, simulation, canary

### 2.1 RPC + wallet (read-only)

```bash
solana --url <YOUR_RPC> health-check          # or: curl getHealth
solana --url <YOUR_RPC> balance <WALLET_PUBKEY>
```

The sniper/copy modules read the wallet balance over RPC on every sizing
decision in simulate/live mode; the cached-balance fallback exists ONLY in
paper mode (`sol_balance_fallback`). With an unreachable RPC in live mode,
sizing fails with a typed RPC error instead of trading on stale numbers.

### 2.2 Simulate mode (real RPC, zero broadcast)

```bash
EXECUTION_MODE=simulate CONFIG_PATH=./config.toml ./target/release/sniper-suite
```

Transactions are built and sent through `simulateTransaction`; the executor
never broadcasts. Verdicts and timings appear in `/api/status`, `/metrics`
(`bot_*` counters) and the JSONL journal.

### 2.3 **FUNDED STEP (operator-controlled)** — devnet canary

Optional, valueless: with a devnet keypair funded from the faucet, broadcast
a self-transfer to prove submission → confirmation → reconciliation
end-to-end. The repo's gated `landing_rate` bench
(`E2E_NETWORK=1 E2E_LIVE=1`, devnet) broadcasts valueless self-transfers
for exactly this purpose — run it only if you accept devnet broadcasts:

```bash
E2E_NETWORK=1 E2E_LIVE=1 E2E_URL=https://api.devnet.solana.com \
  cargo test -p solana-kit --test latency_bench landing_rate -- --nocapture --test-threads=1
```

### 2.4 Confirmation + reconciliation

After any canary or live transaction: `/api/transactions` shows the
signature + confirmation state; the reconciliation loop re-checks ambiguous
executions against chain truth (`docs/RECONCILIATION.md`). A crash mid-flow
is recovered by startup reconciliation (`docs/BACKUP-RESTORE.md` §3).

## 3. What was executed in the delivery sandbox (labels matter)

* `live read`-class checks for Polymarket: implemented and unit/mocked-tested
  (66+5 module tests); NOT executed against Polygon mainnet from the sandbox.
* Solana `simulate`-class machinery: mock-tested; devnet read-only e2e
  (`devnet_e2e.rs`) gate-skips without `E2E_NETWORK=1`.
* Staking program: EXECUTED on a real BPF VM (`solana-test-validator`
  2.1.21) incl. a CPI against the real mainnet-cloned mpl-token-metadata —
  see `docs/STAKING.md` and `docs/EVIDENCE-INDEX.md`. This is local-validator
  evidence, NOT a mainnet deployment.
* No funded live order, canary, or mainnet deployment was executed by the
  vendor. Those remain operator actions.
