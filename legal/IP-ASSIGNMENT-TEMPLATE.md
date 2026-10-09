# INTELLECTUAL-PROPERTY ASSIGNMENT AGREEMENT — TEMPLATE

> **STATUS: TEMPLATE — NOT LEGAL ADVICE.** Drafted for counsel to review,
> complete, and adapt to the governing jurisdiction before signature. The
> bracketed `[PLACEHOLDERS]` must all be resolved. Do not execute this
> document without qualified legal advice in the relevant jurisdiction(s).

**Effective date:** [DATE]

## Parties

1. **Assignor (Seller):** [SELLER LEGAL ENTITY NAME], a [entity type] organized
   under the laws of [JURISDICTION], registered number [NUMBER], address
   [ADDRESS] ("Assignor").
2. **Assignee (Buyer):** [BUYER LEGAL ENTITY NAME], a [entity type] organized
   under the laws of [JURISDICTION], registered number [NUMBER], address
   [ADDRESS] ("Assignee").

## Recitals

A. Assignor is the sole author and owner of all right, title, and interest in
   and to the software product known as **Sniper-Suite** as identified in
   **Schedule 1** (the "Assigned Materials"), except for Third-Party
   Components listed in `legal/THIRD-PARTY-NOTICES.md`.

B. Assignee has agreed to purchase, and Assignor has agreed to assign, all of
   Assignor's intellectual-property rights in the Assigned Materials on the
   terms of this Agreement and the Source Code Bill of Sale dated [DATE].

## 1. Definitions

- **"Assigned Materials"** means the source code, object code, documentation,
  migration files, infrastructure definitions, test suites, evidence packs,
  and associated materials listed in Schedule 1, including all prior versions
  held in Assignor's repository history up to commit `[RELEASE COMMIT SHA]`.
- **"Third-Party Components"** means the open-source components listed in
  `legal/THIRD-PARTY-NOTICES.md`, which remain under their original licenses
  and are NOT assigned.
- **"Intellectual Property Rights"** means all patents, copyrights, trade
  secrets, database rights, design rights, mask-work rights, moral rights
  (to the extent assignable), and all applications, renewals, and extensions
  of any of these, in any jurisdiction.

## 2. Assignment

2.1 With effect from the Effective Date, Assignor hereby irrevocably assigns
    to Assignee, with full title guarantee and free of all third-party claims,
    all of Assignor's Intellectual Property Rights in and to the Assigned
    Materials, including the right to sue for past infringements.

2.2 The assignment expressly EXCLUDES the Third-Party Components, which
    Assignee receives only under their original open-source licenses as
    described in `legal/THIRD-PARTY-NOTICES.md`.

2.3 Assignor assigns the full copyright in the Assigned Materials for their
    full term, throughout the world, together with all rights of any nature
    in them, including the right to receive royalties and other payments.

## 3. Further assurance

Assignor will, at Assignee's reasonable request and cost, execute all
documents and do all things reasonably necessary to give full effect to this
assignment, including recordal with any intellectual-property office.

## 4. Moral rights

To the maximum extent permitted by law, Assignor (and, where applicable, its
employees and contractors) waives and agrees not to assert any moral rights
in the Assigned Materials against Assignee or its successors.

## 5. Representations and warranties

Assignor represents and warrants that:

a. it is the sole beneficial owner of the Assigned Materials and has full
   right and authority to enter into this Agreement;
b. the Assigned Materials do not, to Assignor's knowledge, infringe any
   third-party intellectual-property rights, EXCEPT that no warranty is given
   beyond the open-source licenses recorded in `legal/THIRD-PARTY-NOTICES.md`;
c. the Assigned Materials are free of encumbrances, liens, and licenses other
   than those disclosed in Schedule 2;
d. no contributor to the Assigned Materials has retained rights that would
   conflict with this assignment (all contractor/employee contributions were
   made under work-for-hire or prior assignment arrangements);
e. it has no knowledge of any pending or threatened claim concerning the
   Assigned Materials.

EXCEPT AS EXPRESSLY STATED, THE ASSIGNED MATERIALS ARE TRANSFERRED "AS IS";
ASSIGNOR MAKES NO WARRANTY OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
PURPOSE, OR TRADING PERFORMANCE.

## 6. Non-assert / retained-use carve-out (NEGOTIATE — delete if not agreed)

[OPTIONAL] Assignor retains a non-exclusive, non-transferable internal-use
right to the general techniques (but not the source code) described in the
Assigned Materials for purposes unrelated to Assignee's market. [OR DELETE
ENTIRELY FOR A FULL EXIT.]

## 7. Confidentiality

The terms of this Agreement are confidential for [3] years, except as
required by law, regulation, or court order, or to enforce this Agreement.

## 8. Governing law and disputes

[GOVERNING LAW]. The parties submit to the exclusive jurisdiction of the
courts of [FORUM]. [ARBITRATION ALTERNATIVE.]

## 9. Entire agreement

This Agreement, together with the Source Code Bill of Sale and the schedules,
constitutes the entire agreement between the parties on its subject matter.

---

**SIGNED**

| | Assignor | Assignee |
|---|----------|----------|
| Name | [FULL NAME] | [FULL NAME] |
| Title | [TITLE] | [TITLE] |
| Signature | _________________ | _________________ |
| Date | [DATE] | [DATE] |

## Schedule 1 — Assigned Materials

- Repository snapshot at commit `[RELEASE COMMIT SHA]` (workspace crates,
  control-plane application, `programs/staking-suite`, migrations, docs,
  scripts, evidence packs), EXCLUDING the files under `docs/archive/`.
- All deployment runbooks and configuration templates shipped in the package.
- The domain names / accounts listed here, if included in the sale: [LIST].

## Schedule 2 — Disclosed encumbrances / third-party licenses

- Third-Party Components per `legal/THIRD-PARTY-NOTICES.md` (707 packages;
  MIT, Apache-2.0, MPL-2.0, BSD, Unicode-3.0 and similar permissive terms).
- Hosted-service dependencies that require the buyer's own accounts (RPC
  providers, Stripe, Polymarket CLOB API keys, Jito, SMTP/email providers).
