# OWNER-TODO (p0g) — 2026-10-09

## BLOCKED(owner) inputs
- OWNER_LEGAL_NAME is empty. Needed for G.11 (package) and every [SELLER LEGAL ENTITY NAME] placeholder.
- ADJUDICATE_IDENTITY_PLANE = "no". B-SANCT (11 identity-plane class-4 SQL entries) stays open until you decide. Options: "yes" with the sanction list, or redesign the identity plane.
- EXTRAS = "no". Helm chart and branding config not built.

## Decisions needed (default in brackets)
1. Business matrix 89% vs 100% (docs/BUSINESS-MATRIX-2026.md): is docs/BUYER-ACCEPTANCE-TEST.md accepted? [default: accept, regenerate to 100%].
2. Secret scanners disagree: final-release-check flags the detector string in crates/core/src/custody/credentials.rs; scan-secrets does not. [default: scanner allow-list entry with reason].
3. gates_risk_intel expectations (5 tests): code or test is wrong? [default: no change until you approve].
4. missing_liquidity label (InsufficientLiquidity vs ExecutionUnavailable). [default: relabel code].
5. Staking S-3 (top-up resets cooldown?) [default: reset on every top-up, matching INVARIANTS D6].
6. Consent record for sign-up: backend consent table does not exist. [default: list as backend gap].

## Owner work (outside the agent's reach)
- Counsel documents: licence or IP assignment, terms, privacy, risk disclosure. Agent must not write legal text.
- A real security contact in SECURITY.md.
- Staking program keypair, declare_id! and deployment.
- Live validation runs in order with spend caps (evidence/live 11 of 11 PASSED).
- Credentials to rotate: none flagged by the history scan; rotate any key you have ever pasted into chat.
- Independent audit and penetration test reports in evidence/audits/.

## CI round-trip
1. `git push -u origin p0g` from /home/user/Sniper-Bot-Copy-Trading-Bot.
2. Open a pull request p0g -> main.
3. Copy the first failing step of each failed job (about 80 lines) to ~/ci-logs/<job>.txt, or paste it to the agent, then send `RESUME P0G`.

## PROPOSED branch plan (NOT executed)
```
git fetch origin
git switch main
git merge --ff-only origin/main
git merge --no-ff p0g -m "integrate p0g: static gates, B-SQL, regenerated stats"
git push origin main   # only after CI is green on the pull request
```
Default branch detected: main (origin/HEAD -> origin/main).

## Deferred (D6) and out of scope
- Deferred: safety-gate wiring (M1), BroadcastMode::Race (M2), platform fee (M3), limit/DCA/pools/referrals (M4).
- Out of scope: Raydium CPMM / LaunchLab, Meteora adapters; Polymarket copy/leaders/builder codes; Telegram /buy and /sell; SSO/OIDC; backtest simulator; staking fuzz harness; any live, funded or credentialed call.
