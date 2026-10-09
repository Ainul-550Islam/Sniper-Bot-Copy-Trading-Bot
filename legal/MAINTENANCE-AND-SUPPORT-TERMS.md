# MAINTENANCE AND SUPPORT TERMS — TEMPLATE

> **STATUS: TEMPLATE — NOT LEGAL ADVICE.** For counsel to review and adapt.
> Attach to the Source Code Bill of Sale when a paid support window is
> purchased. `[PLACEHOLDERS]` must be resolved.

**Effective date:** [DATE] · **Term:** [12] months ("Support Period")

## 1. Parties

Seller and Buyer as defined in the Source Code Bill of Sale dated [DATE].

## 2. Support scope

During the Support Period, Seller will provide:

a. **Defect support.** Investigation and fix (or documented workaround) of
   reproducible defects in the Package as delivered.
b. **Protocol-drift fixes.** Patches required when a THIRD-PARTY protocol or
   API the Package depends on ships a breaking change, per the response SLAs
   in Section 4. Covered protocols: pump.fun / PumpSwap, Raydium, Polymarket
   CLOB, Jito, SPL Token. New protocols are out of scope unless added by
   written change order.
c. **Security advisories.** Notice of any vulnerability Seller discovers in
   the delivered code, with a remediation plan.
d. **Knowledge transfer.** Up to [40] hours of engineering time for
   onboarding, architecture walkthrough, and deployment assistance.

OUT OF SCOPE: new features, new venues/exchanges, customization, hosting or
operation of the software on Buyer's behalf, and anything requiring a new
regulatory determination.

## 3. Request channels

- Severity 1 (production down / funds at risk): [24x7 phone or pager] +
  ticket.
- Severity 2 (degraded, workaround exists): ticket via [TRACKER], business
  hours.
- Severity 3 (question / cosmetic): ticket, best-effort.

## 4. Response and resolution SLAs (NEGOTIATE — illustrative)

| Severity | First response | Target resolution or workaround |
|----------|----------------|----------------------------------|
| 1 — protocol-drift that blocks trading | [4] hours, 24x7 | [48] hours |
| 1 — other production down | [8] hours, 24x7 | [5] business days |
| 2 | [2] business days | [10] business days |
| 3 | [5] business days | best effort |

If Seller misses an SLA twice in a calendar quarter for the same severity,
Buyer receives a credit of [10]% of the quarterly support fee.

## 5. Credential escrow and handover

5.1 At contract start, the parties will complete the credential handover
    listed in Schedule 1. Seller retains NO copies of Buyer's production
    credentials after handover is signed off.
5.2 Any secret Seller must access during the Support Period is provided by
    Buyer through a scoped, revocable mechanism (least privilege, time-boxed)
    and is never stored by Seller beyond the session.

## 6. Escrow (OPTIONAL — for assignment-model deals)

[If agreed: the delivered source is placed with escrow agent [NAME] under the
terms of Escrow Agreement dated [DATE], released to Buyer on Seller's
insolvency, material breach, or cessation of the support obligation.]

## 7. Fees and payment

Support fee: [AMOUNT + CURRENCY] per [quarter], payable in advance. Late
payment accrues [1.5]% per month. Seller may suspend support for amounts
overdue by more than [30] days after written notice.

## 8. No warranty extension

Support does not extend, revive, or create any warranty beyond the Bill of
Sale. Fixes are provided with reasonable skill and care; the "AS IS"
disclaimer survives.

## 9. Confidentiality and data protection

Each party protects the other's confidential information with at least
reasonable care. Neither party processes the other's personal data except as
necessary to perform this agreement; where Buyer shares user data for a
support case, it is pseudonymized first.

## 10. Term, renewal, termination

This agreement runs for the Support Period and renews only by written
agreement. Either party may terminate on [30] days' written notice for
material breach uncured after notice. Sections 5, 8, 9, and 11 survive.

## 11. Governing law

Same governing law and forum as the Bill of Sale unless stated here:
[GOVERNING LAW / FORUM].

---

**SIGNED** (same signature block format as the Bill of Sale)

## Schedule 1 — Credential handover checklist

| Item | Owner before | Owner after | Revoked by Seller on |
|------|--------------|-------------|----------------------|
| RPC provider API keys | Seller | Buyer | handover sign-off |
| Jito tip / auth keys | Seller | Buyer | handover sign-off |
| Polymarket CLOB API credentials | Seller | Buyer | handover sign-off |
| Stripe account + webhook secrets | Seller | Buyer | handover sign-off |
| SMTP / email provider keys | Seller | Buyer | handover sign-off |
| KMS / Vault keys & policies | Seller | Buyer | handover sign-off |
| Treasury / operator wallet keys | Seller | Buyer | handover sign-off |
| Domain / DNS / certificate accounts | Seller | Buyer | handover sign-off |
