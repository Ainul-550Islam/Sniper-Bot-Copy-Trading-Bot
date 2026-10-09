# COMMERCIAL INTELLECTUAL PROPERTY TRANSFER & ASSIGNMENT AGREEMENT

**Document Version:** 1.0.0  
**Effective Date:** 2026-10-05  
**Product:** Sniper Suite Enterprise Trading Platform  

---

## 1. Transfer of Rights & Intellectual Property

Upon execution of the commercial transaction and receipt of the agreed purchase consideration ($30,000 – $60,000 USD or equivalent), the Seller irrevocably transfers, assigns, and conveys to the Buyer all worldwide right, title, and interest in and to:

1. **Source Code & Architecture:** All proprietary Rust crates (`server`, `core`, `solana-kit`, `module-sniper`, `module-copy`, `module-polymarket`), smart contracts (`staking-suite`), and Next.js 16 web applications (`apps/control-plane`).
2. **Database Schemas & Migrations:** The complete database migration suite (Migrations `0001` through `0038`).
3. **Documentation, Runbooks & Specifications:** All technical specifications, disaster recovery runbooks, architecture diagrams, and data room assets.
4. **Build & Deployment Pipelines:** Docker configurations, CI/CD workflows, backup/restore scripts, and automation toolchains.

---

## 2. Representations and Warranties

The transferring party represents and warrants that:
- **Clean Chain of Title:** The source code contains no unauthorized third-party proprietary code.
- **Open Source Compliance:** All third-party dependencies are licensed under permissive open-source licenses (MIT, Apache-2.0, BSD-3-Clause) as verified in `sbom.cyclonedx.json` and `licenses.json`.
- **Zero Malicious Code:** The codebase contains no intentional backdoors, telemetry spyware, or unauthorized logic.
- **Fail-Closed Custody:** The custody architecture requires explicit KMS/signer authorization and maintains complete tenant isolation.

---

## 3. Commercial Warranty & Non-Exclusivity Boundary

- **As-Is Delivery:** The software is delivered as an enterprise engineering asset. Production operation on live mainnet networks requires the Buyer to supply their own live RPC endpoints, cloud infrastructure credentials, and API keys.
- **Full Source Access:** The Buyer receives full source code access with unrestricted rights to modify, rebrand, white-label, host, sub-license, or commercialize the platform.
