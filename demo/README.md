# Esmeralda demo: one full lane

The manifests in this folder ran on Esmeralda on 2026-10-26 against `deployments/esmeralda.json`,
with a demo provider and a demo buyer account in a local wallet daemon. Each was dry-run first with
`scripts/submit.py`, then submitted with `--live`.

| Step | Manifest | Signed by | Transaction | Result |
|---|---|---|---|---|
| Fund demo accounts (2 tTARI each) | `00-fund-demo-accounts.rs` | admin | `f7b5ea70…4722` | accounts created on-chain |
| Provider registers, policy "anyone" | `01-register-provider.rs` | provider | `7860d44e…a229` | `lanes.provider_registered` |
| Buyer deposits 1 tTARI, opens lane 1 with 0.5 tTARI and a hot voucher key | `02-deposit-open-lane.rs` | buyer | `55da8e0c…154c` | `lanes.lane_opened`, lane 1 |
| Provider claims a JS-signed voucher for 0.2 | `03-claim.rs` (with 200000) | provider | `29f6e7d9…3178` | paid 0.2 |
| Replay of the same voucher | `03-claim.rs` (with 200000) | provider | dry run only | rejected: *voucher does not exceed the amount already claimed* |
| Provider claims cumulative 0.35 | `03-claim.rs` | provider | `fc65e90a…48ad` | paid 0.15 |
| Provider releases the lane | `04-release-lane.rs` | provider | `7b265412…cfea9` | 0.15 back to the buyer's ledger balance |

Final on-chain state: buyer ledger balance 650000 µT, lane 1 deposited 500000 / claimed 350000 /
closed, fees owed 0, ledger vault 650000. The provider account went from 2.000000 to 2.333883 tTARI
(+0.35 claimed, −0.016117 in four transaction fees).

Fees seen: about 1.2–1.8 tTARI to publish a template, 0.003–0.007 tTARI per call.
