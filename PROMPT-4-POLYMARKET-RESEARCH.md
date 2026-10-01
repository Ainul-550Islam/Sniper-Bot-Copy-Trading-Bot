# PROMPT 4/10 — §D Polymarket V3/async protocol research notes

Research date: 2026-09-30. Sources: official Polymarket docs + official client
repos (see per-fact citations). These notes are the "actual current
wire/protocol semantics" the spec requires §D to implement against —
`@polymarket/clob-client-v2` v1.2.0 is the dependency reference named by the
spec.

## 1. The three-protocol landscape (do not conflate)

| layer | what it is | status |
|---|---|---|
| **CLOB V2** (2026-04-28 cutover) | rewritten CLOB backend + new Exchange contracts + pUSD collateral; EIP-712 Exchange domain version `"2"`; V1-signed orders rejected (`Invalid order payload`) | live on `https://clob.polymarket.com` (the `clob-v2.polymarket.com` host was pre-cutover testing only, NO LONGER the integration target) |
| **Exchange V3** | position-backed orders: PolyV2 markets expose **position IDs** instead of CTF token IDs; domain version `"3"`, dedicated verifying contract | live; selected automatically when the order identifies a position |
| **async commit pipeline** | `POST /order` acceptance no longer implies settlement hashes: `delayed` status, `tradeIDs`, `transactionsHashes` resolved later via polling/WS | live (Jul 2026 client behavior) |

The current `module-polymarket` implements CLOB V2 signing correctly (11-field
Order struct, domain version `"2"`) but has **no V3 (position-backed) path and
no async-response/backfill surface** — audit P0 #5 confirmed.

## 2. CLOB V2 exact semantics (already correct in `eip712.rs`; keep)

- Order struct (11 fields): `Order(uint256 salt,address maker,address signer,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint8 side,uint8 signatureType,uint256 timestamp,bytes32 metadata,bytes32 builder)` — `taker`, `expiration`, `nonce`, `feeRateBps` REMOVED.
- Domain: `EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)`, name `"Polymarket CTF Exchange"`, version `"2"`, chainId 137.
  - standard verifyingContract: `0xE111180000d2663C0091e4f400237545B87B996B`
  - neg-risk verifyingContract: `0xe2222d279d744050d28e00520010520000310F59`
- `timestamp` = order creation time in **milliseconds**; replaces `nonce` for per-address uniqueness (NOT an expiration).
- `side` encoded `uint8` (0=BUY, 1=SELL) in the signed payload; wire body uses strings.
- `metadata`, `builder` are bytes32; builder zero unless attaching a builder code.
- `expiration` remains in the POST /order **wire body** for GTD but is NOT signed.
- Fees: operator-set at match time (taker-only, dynamic per market via `getClobMarketInfo()` → `fd`); NOT embedded in the signed order. Makers never pay.
- Collateral: pUSD (ERC-20 backed by USDC); power users wrap USDC.e→pUSD via Collateral Onramp `wrap()`.
- Builder attribution: single `builderCode`/`builder` field on the order; `POLY_BUILDER_*` HMAC headers GONE (builder API key still used only for the Relayer/gasless flow).
- `ClobAuthDomain` (L1 API auth) UNCHANGED — version `"1"`.
- Order types: GTC / GTD / FOK / FAK; `postOnly` works with GTC/GTD only; `orderType` is top-level body, not signed.

## 3. Exchange V3 — position-backed orders (§D new surface)

- **Discriminator:** an order carries exactly ONE of `tokenID` (CTF token-backed) or `positionID` (PolyV2 position-backed). Supplying `positionID` selects Exchange V3 signing automatically and **disables neg-risk routing** (V3 has a single verifying contract; no neg-risk variant). clob-client-v2 v1.2.0 validates "exactly one non-empty identifier" at runtime (their #110; earlier #104 noted the TS-type-only gap where both set silently chose position — replicate the runtime validation, do NOT rely on types).
- **Signed/wire field name stays `tokenId`** even when it carries a position id.
- V3 domain: name `"Polymarket CTF Exchange"`, version `"3"`, chainId 137, verifyingContract `0xe3333700cA9d93003F00f0F71f8515005F6c00Aa`.
- V3 Order type: same 11 fields as V2 (`salt, maker, signer, tokenId, makerAmount, takerAmount, side, signatureType, timestamp, metadata, builder`) — the difference is the domain (version + verifying contract), NOT the struct.
- `signatureType`: 0=EOA, 1=POLY_PROXY, 2=GNOSIS_SAFE, 3=POLY_1271 (deposit wallet). For type 3 the signature wraps the inner EIP-712 sig with `appDomainSeparator || contentsHash || ORDER_TYPE || len` (TypedDataSign wrapper with DepositWallet domain) — only needed if §D supports deposit-wallet signers; EOA path is the direct order digest.
- Docs discrepancy to record honestly: the Combos/RFQ doc labels V3 `timestamp` "<unix_seconds>" while the CLOB V2 migration doc + POST /order examples use **milliseconds**. clob-client-v2 (the dependency reference) uses milliseconds for CLOB orders → implement ms, document the discrepancy.

## 4. Async commit pipeline (§D new surface)

`POST /order` response (also per-element for `POST /orders` batch, max 15):

```json
{
  "success": true,
  "orderID": "0x…",
  "status": "matched | live | delayed | unmatched",
  "makingAmount": "…", "takingAmount": "…",
  "transactionsHashes": ["0x…"],
  "tradeIDs": ["…"],
  "errorMsg": ""
}
```

- `matched` — filled (fully/partially); `tradeIDs` identifies the trades; `transactionsHashes` carries settlement hashes **when available** (may be absent — settlement is asynchronous).
- `live` — resting on the book.
- `delayed` — ACCEPTED but not matched yet (market has a matching delay; detect via `market.trading.secondsDelay`). `makingAmount`/`takingAmount` are `"0"`, `tradeIDs`/`transactionsHashes` EMPTY. Treat as PENDING, follow via real-time order updates (WS user channel) — never as a fill.
- `unmatched` — marketable but placement failed.
- Settlement is asynchronous after a match: an accepted response may lack tx hashes. Backfill = poll `GET /trades` (and order status) until trade status is terminal; client reference behavior: `wait_for_order_fill_settlement` waits for every fill in `tradeIDs` to settle on-chain (default 30s timeout, configurable; TimeoutError does NOT undo executed trades).
- `deferExec: bool` on the POST body asks the CLOB to defer execution.

## 5. Trade records (backfill/reconciliation source)

`GET /trades` fields: `id`, `taker_order_id`, `market` (condition id), `asset_id`, `side`, `size`, `fee_rate_bps`, `price`, `status`, `match_time`, `last_update`, `outcome`, `maker_address` (funder of taker), `owner` (api key), `transaction_hash`, `bucket_index` (trade split across multiple txs), `maker_orders[]`, `type` (TAKER|MAKER).

Trade statuses (terminal in bold): MATCHED (sent to executor) → MINED (seen on chain, no finality threshold) → **CONFIRMED** (final, successful); RETRYING (revert/reorg, being retried); **FAILED** (terminal, not retried).

## 6. Contracts summary (chainId 137)

| contract | address |
|---|---|
| CLOB V2 Exchange (standard) | `0xE111180000d2663C0091e4f400237545B87B996B` |
| CLOB V2 Exchange (neg-risk) | `0xe2222d279d744050d28e00520010520000310F59` |
| Exchange V3 (position-backed) | `0xe3333700cA9d93003F00f0F71f8515005F6c00Aa` |

## 7. Implementation consequences for §D

1. Keep V2 token-backed signing intact (it is correct).
2. Add V3: routing (tokenID XOR positionID, runtime-validated), V3 domain constants, neg-risk disabled on the V3 path, wire field still `tokenId`.
3. Add async response model: `tradeIDs`, `transactionsHashes`, `makingAmount/takingAmount`, `delayed` → PENDING semantics; never fabricate hashes or trade ids.
4. Add backfill/reconciliation: poll trades + order status to terminal; RETRYING→FAILED handling; bucket_index awareness; timeout that does not misreport.
5. Extend, don't duplicate (rule #35): the existing `ClobClient`, `orders.rs` pipeline, `reconcile.rs`, `ws.rs` user channel are the integration points.

## Sources

- https://docs.polymarket.com/v2-migration (CLOB V2 cutover guide; live 2026-04-28)
- https://github.com/Polymarket/clob-client-v2 PR #104 + release v1.2.0 (positionID→V3 routing; exactly-one-identifier validation)
- https://www.npmjs.com/package/@polymarket/clob-client-v2 (v1.2.0 usage; positionID example)
- https://github.com/Polymarket/py-clob-client-v2 (position_id selects V3, ignores neg_risk/exchange-version overrides; wire field stays tokenId)
- https://docs.polymarket.com/trading/orders/create (POST /order body, statuses, tradeIDs/transactionsHashes, delayed semantics, secondsDelay, deferExec)
- https://docs.polymarket.com/trading/combos/market-makers (Exchange V3 domain version "3" + verifyingContract + 11-field Order typed data; deposit-wallet TypedDataSign wrapper)
- https://docs.polymarket.com/api-reference/trade/post-multiple-orders (batch response schema incl. tradeIDs)
- Archived CLOB API docs (trade record schema + MATCHED/MINED/CONFIRMED/RETRYING/FAILED lifecycle)
- pmxt issue #2439 (V2 cutover breakage corroboration: domain version 2, removed fields, pUSD)
