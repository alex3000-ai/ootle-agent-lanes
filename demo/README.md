# Esmeralda run: v0.3 (2026-10-28)

Every step ran with `client/lanes.mjs` against the ledger in `deployments/esmeralda.json`, dry-run first and then
`--live`. The buyer is the account `lanes-admin`; the provider is the directory agent `opencodeagent`. Fees are what
the network charged, in µT.

| Step | Command | Signed by | Transaction | Fee | Result |
|---|---|---|---|---:|---|
| Publish the template | `deploy/publish-ledger.mjs` | `opencodeagent` | `447489ad…6a3d` | 2,563,621 | `template_2d14a6b3…82aa` |
| Create the ledger | same script | `opencodeagent` | `10609712…4ac8` | 3,422 | no owner, 6-epoch close window |
| Open lane 1 with 0.02 tTARI | `open` | buyer | `1aba417d…0aae` | 5,829 | lane 1 records `opencodeagent`'s agent key |
| Claim a voucher for 5,000 | `claim` | provider | `a9626f01…a398` | 4,893 | paid 5,000; a replay is refused before sending |
| Open lanes 2–21 (1 µT each) in one transaction | scratch script | buyer | `b62016e6…e43d` | 59,150 | 21 lanes |
| Open lane 22 with 0.02 tTARI | `open` | buyer | `28d29e69…ccba` | 5,914 | **+85 µT after 21 lanes** |
| Claim 5,000 on lane 22 | `claim` | provider | `8599e8dd…53cb` | 4,894 | **+1 µT against lane 1's claim** |
| Request a close | `close` | buyer | `ea9ebb9d…633d` | 2,862 | refund refused until epoch 12702 |
| Provider releases the lane | `release` | provider | `db97ba3a…02ac` | 2,802 | closed at once |
| Buyer refunds | `refund` | buyer | `308cb9f6…35e7` | 3,688 | 15,000 back to the buyer |
| Top up lane 1 by 5,000 | `fund` | buyer | `91d6f182…e286` | 3,667 | lane 1 holds 20,000 |

Calls cost the same as the ledger fills: about 4 µT more per lane within a page vault of 256, then back down for the
next page. v0.2 added about 155 µT to every call for each lane ever opened (24,225 µT for `open_lane` at 128 lanes).

The v0.2 and v0.1 runs are in this folder's git history.
