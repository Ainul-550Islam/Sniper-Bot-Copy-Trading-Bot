# SOURCE CODE BILL OF SALE — TEMPLATE

> **STATUS: TEMPLATE — NOT LEGAL ADVICE.** Drafted for counsel to review and
> adapt. All bracketed `[PLACEHOLDERS]` must be resolved before signature.
> This document effects the SALE; pair it with EITHER the Proprietary License
> in `LICENSE` (license model) OR `legal/IP-ASSIGNMENT-TEMPLATE.md`
> (assignment model). Counsel must choose and state which model applies.

**Bill of Sale No.:** [NUMBER]
**Effective date:** [DATE]

## 1. Parties

- **Seller:** [SELLER LEGAL ENTITY NAME], [entity type], [JURISDICTION],
  registration [NUMBER], address [ADDRESS] ("Seller").
- **Buyer:** [BUYER LEGAL ENTITY NAME], [entity type], [JURISDICTION],
  registration [NUMBER], address [ADDRESS] ("Buyer").

## 2. Goods sold

Seller sells, and Buyer purchases, all of Seller's right, title, and interest
in the following ("the Package"):

a. the complete source-code repository known as **Sniper-Suite** at commit
   `[RELEASE COMMIT SHA]`, including all workspace crates, the control-plane
   web application, the on-chain program `programs/staking-suite`, database
   migrations, deployment definitions, scripts, documentation, and evidence
   packs;
b. the deliverable archive identified by SHA-256 checksum:
   `[PACKAGE SHA-256]` (verifiable with `scripts/verify-delivery.sh`);
c. the documentation listed in Schedule 1.

EXCLUDED from the sale: (i) Third-Party Components per
`legal/THIRD-PARTY-NOTICES.md`, which pass under their original open-source
licenses only; (ii) Seller's pre-existing tools and know-how not embodied in
the Package; (iii) Seller's trademarks unless separately agreed.

## 3. Purchase price and payment

3.1 Total price: [AMOUNT + CURRENCY] (the "Price"), structured as:
    - deposit of [AMOUNT] payable on signature;
    - balance of [AMOUNT] payable on delivery and acceptance (Section 5).
3.2 Payment by [wire transfer / escrow provider [NAME]] to [ACCOUNT].
3.3 Taxes are Buyer's responsibility unless the law requires otherwise.

## 4. IP transfer mechanism (CHOOSE ONE — counsel to strike the other)

- **[Option A — License model.]** Ownership of the copyright remains with
  Seller; Buyer receives the proprietary license set out in `LICENSE`,
  including the grant and restrictions stated there.
- **[Option B — Assignment model.]** Seller assigns all intellectual-property
  rights in the Package to Buyer under `legal/IP-ASSIGNMENT-TEMPLATE.md`,
  which is executed concurrently with this Bill of Sale.

## 5. Delivery and acceptance

5.1 Seller delivers the Package (archive + checksum + deployment keys/notes)
    within [N] business days of the deposit.
5.2 Buyer has an acceptance period of [15] business days to verify the
    Package against the Evidence Index (`docs/EVIDENCE-INDEX.md`) and to run
    the verification scripts under `scripts/`.
5.3 If Buyer rejects within the acceptance period stating specific defects,
    Seller has [10] business days to cure. If uncured, Buyer may terminate
    and receive a refund of amounts paid, less a reasonable fee for services
    already accepted.
5.4 Acceptance occurs on written notice or on expiry of the acceptance period.

## 6. Seller's limited representations

Seller represents that, to the best of its knowledge: it owns or validly
licenses everything in the Package; the Package does not contain malicious
code, undisclosed back-doors, or time-bombs; and the Third-Party Components
are as listed in `legal/THIRD-PARTY-NOTICES.md`.

## 7. Disclaimer

EXCEPT AS EXPRESSLY STATED IN SECTION 6 AND ANY WRITTEN EVIDENCE PACK, THE
PACKAGE IS SOLD "AS IS". SELLER DISCLAIMS ALL OTHER WARRANTIES, EXPRESS OR
IMPLIED, INCLUDING MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE, AND
NON-INFRINGEMENT. NO REPRESENTATION IS MADE ABOUT TRADING PROFITS, LATENCY,
OR THE CONTINUED OPERATION OF ANY BLOCKCHAIN, EXCHANGE, OR PROTOCOL.
HISTORICAL RESULTS DO NOT GUARANTEE FUTURE PERFORMANCE.

## 8. Limitation of liability

TO THE MAXIMUM EXTENT PERMITTED BY LAW, NEITHER PARTY'S AGGREGATE LIABILITY
EXCEEDS THE PRICE ACTUALLY PAID. NEITHER PARTY IS LIABLE FOR INDIRECT,
INCIDENTAL, CONSEQUENTIAL, OR PUNITIVE DAMAGES, INCLUDING LOST PROFITS OR
TRADING LOSSES.

## 9. Regulatory acknowledgment

Buyer acknowledges that operating the Package may require regulatory
compliance in Buyer's markets (see `legal/REGULATORY-CHECKLIST.md`). Seller
provides no legal, tax, or investment advice, and Buyer's use of the Package
is at Buyer's own regulatory risk.

## 10. Transition services (OPTIONAL — delete if not included)

[Include `legal/MAINTENANCE-AND-SUPPORT-TERMS.md` if a support window is
purchased: support scope, response SLAs, protocol-drift fixes, credential
handover.]

## 11. Governing law and disputes

[GOVERNING LAW]; exclusive jurisdiction of [FORUM]. [ARBITRATION OPTION.]

## 12. Entire agreement; counterparts

This Bill of Sale, with its schedules and the documents it incorporates, is
the entire agreement on its subject matter and may be executed in
counterparts, including electronic signatures.

---

**SIGNED**

| | Seller | Buyer |
|---|--------|-------|
| Name | [FULL NAME] | [FULL NAME] |
| Title | [TITLE] | [TITLE] |
| Signature | _________________ | _________________ |
| Date | [DATE] | [DATE] |

## Schedule 1 — Documentation delivered

- `README.md`, `CHANGELOG.md`, `SECURITY.md`, `docs/ARCHITECTURE.md`,
  `docs/OPERATIONS.md`, `docs/SECURITY.md`, `docs/DEPLOYMENT.md`,
  `docs/API.md`, `docs/MODULES.md`, `docs/TESTING.md`,
  `docs/KNOWN-LIMITATIONS.md`, `docs/EVIDENCE-INDEX.md`,
  `docs/BUYER-HANDOVER.md`, `legal/*`, `openapi/openapi.json`.
