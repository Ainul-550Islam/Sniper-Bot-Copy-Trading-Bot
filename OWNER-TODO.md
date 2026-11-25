# Owner-only actions

These actions require the owner's legal judgment, authorization, credentials,
or external parties. Commands open the relevant instructions for review; they
 do not claim that external validation has been performed.

| Action | Exact command |
|---|---|
| Have counsel review legal identity, IP transfer, terms, privacy and risk disclosure | `${EDITOR:-vi} docs/IP-OWNERSHIP-REGISTER.md docs/COMMERCIAL-IP-TRANSFER-AGREEMENT.md` |
| Set a real security reporting contact | `${EDITOR:-vi} SECURITY.md` |
| Provision a staking program keypair and review deployment steps before deploying | `./scripts/staking-identity.sh verify && sed -n '1,45p' scripts/staking-identity.sh` |
| Arrange an independent audit and penetration test; record findings | `${EDITOR:-vi} docs/SECURITY.md` |
| Decide whether to rewrite published history; review affected paths first (force-push requires separate approval) | `git log --all --oneline -- .cargo/bin .cargo/env` |
| Review externally credentialed validation instructions and set appropriate spend caps | `sed -n '1,200p' docs/EXTERNAL-VALIDATION-RUNBOOK.md` |
| Review and approve promotion only after the appropriate CI checks | `git status --short && git remote -v` |
