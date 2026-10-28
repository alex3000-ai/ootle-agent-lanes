# Esmeralda demo: one full lane (v0.2)

These steps ran on Esmeralda on 2026-10-28 against `deployments/esmeralda.json`, with accounts in a
local wallet daemon. The provider is the directory agent `opencodeagent`; the buyer is a separate
account. Each step was dry-run first, then sent with `--live`.

| Step | How | Signed by | Transaction | Result |
|---|---|---|---|---|
| Provider publishes its listing | `client/lanes.mjs set-listing` | `opencodeagent`'s agent key | `6f1562e9…07ae` | listing = its Ootlebook service page |
| Buyer deposits 1 tTARI | `02-deposit.rs` | buyer | `3519c7b7…3c86` | ledger balance 1000000 µT |
| Buyer opens lane 1 to `opencodeagent` with 0.5 tTARI and a hot voucher key | `client/lanes.mjs open-lane` | buyer | `cfb2a80b…176c` | lane 1, provider key = the card's agent key |
| Provider claims a JS-signed voucher for 0.2 | `03-claim.rs` | provider | `0001ee2b…5ba0` | paid 0.2 |
| Replay of the same voucher | `03-claim.rs` | provider | dry run only | rejected: *voucher does not exceed the amount already claimed* |
| Provider releases the lane | `04-release-lane.rs` | provider | `4fcfc613…01a1` | 0.3 back to the buyer's ledger balance |

Final on-chain state: buyer ledger balance 800000 µT, lane 1 deposited 500000 / claimed 200000 /
closed, ledger vault 800000.

Fees seen: 2.07 tTARI to publish the template, 0.0023 to create the ledger, 0.003–0.005 per call.

The first run, on v0.1 (2026-10-26), is in this folder's git history.
