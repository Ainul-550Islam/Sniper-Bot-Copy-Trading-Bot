# REGULATORY CHECKLIST — TEMPLATE FOR THE BUYER'S COUNSEL

> **STATUS: TEMPLATE — NOT LEGAL ADVICE.** This is a question list for the
> buyer's counsel to work through BEFORE deploying the Software commercially
> in any market. The seller provides no legal advice; unanswered items are a
> buyer-side risk, not a deliverable gap. Date completed: [DATE].
> Counsel of record: [NAME / FIRM].

## 1. What the Software does (factual basis for counsel)

- Automated trading client: snipes new Solana token launches (pump.fun /
  PumpSwap / Raydium), copy-trades selected wallets, and trades Polymarket
  prediction markets via its CLOB API.
- Multi-tenant SaaS control plane (auth, billing via Stripe, per-tenant
  wallets, kill switches) and an on-chain Solana staking program.
- It executes trades with REAL funds. It does not itself custody third-party
  funds in the SaaS build, but tenant wallet-key configurations determine
  custody reality in each deployment — counsel must evaluate the ACTUAL
  deployment topology.

## 2. Market-access questions (per target jurisdiction)

For EACH country where the buyer will offer or operate the Software:

- [ ] Is automated trading software lawful for retail/professional users
      here? Any licensing (broker-dealer, investment-adviser, commodity
      trading advisor) triggered by selling signals, copy-trading, or
      "strategy" products?
- [ ] Does marketing the Software require financial-promotion approvals
      (e.g., UK FCA financial-promotion rules; EU MiFID inducement rules)?
- [ ] Are there foreign-exchange or capital-control restrictions on users
      funding trading wallets (relevant, among others, in Bangladesh, India,
      Nigeria)?
- [ ] Sanctions screening: are users screened against OFAC / EU / UN / local
      sanctions lists before onboarding and before payment acceptance?

## 3. Prediction markets — Polymarket-specific

- [ ] Polymarket's own terms restrict access from certain jurisdictions and
      for certain persons (historically including United States persons for
      trading; the buyer's counsel must confirm the CURRENT restricted-list
      on Polymarket's site and in its CLOB API terms, and geoblock
      accordingly).
- [ ] In the buyer's market, are prediction-market contracts regulated as
      gambling, as derivatives (e.g., CFTC event contracts in the US), or as
      securities? Which regulator must the buyer engage?
- [ ] Does facilitating user access to Polymarket make the buyer an
      unlicensed introducing broker / gambling operator locally?
- [ ] Are winnings/losses reportable for users; what records must the buyer
      keep (tax lot, counterparty, timestamps)?

## 4. Token-launch sniping (Solana memecoin venues)

- [ ] Does operating or selling a sniping bot conflict with any local rule on
      market manipulation, front-running disclosure, or unfair trading
      practices?
- [ ] If the buyer or its users create/promote tokens with the Software, do
      securities laws apply to those tokens (Howey-style analysis in the US;
      MiCA crypto-asset classification in the EU)?
- [ ] Are there consumer-protection duties when selling software that trades
      highly volatile assets to retail users (risk disclosure, no profit
      promises)? The UI's legal pages (`apps/control-plane/src/app/legal/`)
      must carry counsel-approved risk disclosures BEFORE launch.

## 5. Money-transmission / custody

- [ ] Does any deployment topology hold user keys or move user funds on
      instruction? If yes: money-transmitter / payment-institution /
      e-money licensing analysis per jurisdiction (US FinCEN + state MTLs;
      EU PSD2/PSA2 or MiCA CASP; UK FCA registration; SG MAS MAS Act;
      Bangladesh Bank approval where applicable).
- [ ] If the buyer offers hosted wallets: travel-rule (FATF Rec. 16)
      compliance, AML/KYC program, suspicious-activity reporting.
- [ ] Self-custody mode (user holds keys; software only signs locally):
      confirm the licensing analysis is materially different and document it.

## 6. Securities / investment-advice exposure

- [ ] Copy-trading and "leaders" features: does selecting/featuring leaders
      constitute investment advice or a collective-investment scheme?
- [ ] Staking program (`programs/staking-suite`): is the staked token or the
      reward stream a security in the target market? Is operating it a
      regulated activity?
- [ ] Any marketing that implies guaranteed returns must be removed — the
      claims gate (`scripts/verify-marketing-claims.sh`) enforces this in
      docs/UI, but counsel must also review paid advertising.

## 7. Data protection

- [ ] GDPR / UK GDPR (if EU/UK users): DPA, record of processing, cookie
      consent, sub-processor list (RPC providers, Stripe, email provider).
- [ ] Local equivalents for other markets (e.g., Bangladesh's Cyber Security
      Act / data-localization questions).
- [ ] Wallet addresses are pseudonymous but can become personal data when
      linked to accounts — treat them as personal data in the SaaS plane.

## 8. Tax

- [ ] Character of SaaS subscription revenue vs. per-trade platform fees.
- [ ] User trading gains: does the platform have withholding/reporting
      duties (e.g., DAC8 crypto-asset reporting in the EU; US broker
      reporting rules when applicable)?

## 9. Export controls and dual-use

- [ ] Cryptographic software export classification (US EAR/Wassenaar;
      note the TSU/open-source exemptions and their limits).

## 10. Sign-off

| Question area | Counsel conclusion (attach memo) | Owner | Date |
|---|---|---|---|
| 2 Market access | | | |
| 3 Prediction markets | | | |
| 4 Token sniping | | | |
| 5 Money transmission / custody | | | |
| 6 Securities | | | |
| 7 Data protection | | | |
| 8 Tax | | | |
| 9 Export controls | | | |

> Items left blank are OPEN RISKS. The buyer should not launch in a market
> with open risks.
