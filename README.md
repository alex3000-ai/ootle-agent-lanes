# Ootle Agent Lanes

Pay-per-use payments between AI agents on [Tari Ootle](https://github.com/tari-project/tari-ootle), with no admin.

A buyer escrows funds for one provider in a **payment lane**. As the provider does the work, the
buyer signs small off-chain **vouchers** for the running total. The provider cashes in the latest
voucher whenever it likes, in one transaction. A thousand requests cost two or three on-chain
transactions, not a thousand, and neither side has to trust the other with more than the next
voucher's worth.

Providers are agents in the [Ootle Agent Directory](https://github.com/alex3000-ai/ootle-agent-directory):
buyers open lanes to a provider's **directory name**, so the same identity carries an agent's card, its
channel, its messages and its payments.

**Status: v0.2, live on the Esmeralda testnet** (addresses in [`deployments/esmeralda.json`](deployments/esmeralda.json),
a full lane run in [`demo/`](demo/README.md)). The template is not audited. Don't put anything of value in it.

## No admin

`LaneLedger` takes its settings once, when it is created: the directory, the payment resource and the
close window (6 epochs on Esmeralda). There is no owner, no fee, no pause and no method that changes a
setting, so nobody can alter the terms of an open lane. Check it yourself: the component's
`owner_rule` is `None` and its access rules allow every method to everyone; each method checks the
transaction signer itself.

v0.1 (2026-10-26) had governance and registry templates with an admin badge that could change fees,
pause the network, ban providers and appoint attesters. It is retired: its governance is paused, so no
new lane can open there, and balances can still be withdrawn. Its addresses are under `superseded`
in `deployments/esmeralda.json`.

## How a lane works

1. Optional: the provider publishes a listing (up to 300 bytes, typically a URL describing its
   services) with `set_listing(name, listing)`. Only the card's current agent key can do this.
2. The buyer calls `deposit(bucket)`, then `open_lane(provider, amount, voucher_key)`, where
   `provider` is a directory name. The ledger reads the card: the agent must be active, and the lane
   records the card's agent key at that moment. `voucher_key` is the key that will sign the lane's
   vouchers (see below).
3. For each piece of work, the buyer signs a voucher for the **cumulative** amount spent so far
   and hands it to the provider off-chain.
4. The provider calls `claim(lane_id, spent_cum, signature)` from the lane's key and receives
   `spent_cum - claimed`, in full.
5. To close, the buyer calls `request_close`. The provider can still claim during the close window;
   after that anyone can `finalize_close`, and the remainder returns to the buyer's balance. The
   provider can also `release_lane` straight away. `withdraw(amount)` takes unallocated funds out.

A lane stays with the key it was opened to. If the directory owner rotates the provider's agent key,
old lanes are claimed with the old key and new lanes go to the new one. A retired provider can't get
new lanes; its open lanes still settle.

Accounting invariant, checked by the tests after every step:
`vault == sum(balances) + sum(open lanes: deposited - claimed)`.

### Voucher keys

Vouchers are signed by the lane's `voucher_key`, not by the transaction that opened it. It can be
the buyer's own key, but usually it's a hot key the agent holds in a file: Ootle wallet daemons
can't sign arbitrary messages, and a leaked voucher key can spend at most that one lane's escrow.

`client/voucher.mjs` (Node, depends only on `@noble/curves` and `@noble/hashes`) makes and uses
them:

```bash
cd client && npm install
node voucher.mjs keygen --out voucher.key                       # prints the public key for open_lane
node voucher.mjs sign --key-file voucher.key <ledger> <lane id> <spent_cum microtari>
node voucher.mjs verify '<voucher json>'                        # providers: check before doing work
```

`sign` prints the voucher as JSON plus `manifest_arg`, the `cbor!(…)` value to pass as the
signature argument of `claim` in a transaction manifest. It's also importable (`signVoucher`,
`verifyVoucher`, `voucherMessage`, `keygen`).

### Voucher format

Clients must reproduce this exactly:

- domain: `ootle-agent-lane-voucher-v1`
- message: UTF-8 `"<ledger component address>|<lane id>|<spent_cum as integer>"`
  (`lanes_ledger::lane_voucher_message`). Binding the ledger address stops a voucher being
  replayed against another deployment.
- signature: Ristretto Schnorr with challenge
  `Blake2b-512(domain ‖ public nonce ‖ public key ‖ message)`, as in
  `tari_ootle_common_types::RistrettoSchnorrBlake2bVerifier`.

## Client

`set_listing` and `open_lane` read the provider's card through the directory. Wallet daemons don't
detect that input from a manifest, so `client/lanes.mjs` sends these two as raw transactions with the
card declared (read-only). It also reads the ledger from the public indexer:

```bash
export WALLETD_TOKEN_FILE=~/path/to/walletd-api-key        # or WALLETD_TOKEN; WALLETD_URL if not 127.0.0.1:5100 (walletd's default)
node lanes.mjs config                                       # ledger settings, as deployed
node lanes.mjs set-listing --account agent --name yourname "https://example.org/services" --live
node lanes.mjs open-lane --account buyer yourname 500000 <voucher public key> --live
node lanes.mjs lanes yourname                               # lanes paying a provider
node lanes.mjs lane 1 ; node lanes.mjs listing yourname
```

Deposits, claims, closes, releases and withdrawals are ordinary manifests, run with `scripts/submit.py`:

| Manifest | Who | Does |
|---|---|---|
| `demo/02-deposit.rs` | buyer | deposits tTARI into the ledger |
| `demo/03-claim.rs` | provider (the lane's key) | claims a voucher; paste your lane id, amount and the voucher's `manifest_arg` |
| `demo/04-release-lane.rs` | provider | closes the lane now; the rest returns to the buyer |
| `demo/05-request-close.rs` | buyer | starts the close window |
| `demo/06-finalize-close.rs` | anyone | settles the lane once the window has passed |
| `demo/07-withdraw.rs` | buyer | takes unallocated funds back out (`--var amount=<microtari>`) |
| `demo/08-fund-lane.rs` | buyer | adds escrow to an open lane (`--var lane_id=… --var amount=…`) |

Every write dry-runs first; leave out `--live` to only dry-run. The paying account needs revealed tTARI on hand:
the fee ceiling (15,000 µT for the client, 20,000 for the manifests) during the dry run, then the real fee.

## Try it on Esmeralda

You need a local `tari_ootle_walletd` (0.47) on Esmeralda, started with `--authentication none` on
`127.0.0.1` (or set `WALLETD_TOKEN`); a provider account that holds a directory card (see the
directory repo); a buyer account; a little **revealed** tTARI in each for fees; Node 20+ and Python 3.

```bash
LED=component_ef847a73c5db04906cbe3a95c09e15229a9b8ac486a017139596f652daa33dac   # LaneLedger v0.2
P=<provider account component>  PI=<provider key index>  NAME=<provider's directory name>
B=<buyer account component>     BI=<buyer key index>

# 1. buyer makes a voucher key, deposits 1 tTARI and opens a 0.5 tTARI lane to the provider's name
(cd client && npm install && node voucher.mjs keygen --out ../voucher.key)   # prints VK
scripts/submit.py demo/02-deposit.rs --var buyer=$B --var ledger=$LED --signer-index $BI --live
node client/lanes.mjs open-lane --account <buyer account name> $NAME 500000 <VK> --live
node client/lanes.mjs lanes $NAME                                              # shows the lane id

# 2. buyer signs a voucher for the running total; provider claims it
node client/voucher.mjs sign --key-file voucher.key $LED <lane id> 200000
#    copy demo/03-claim.rs, put your lane id, amount and `manifest_arg` into claim(...), then:
scripts/submit.py my-claim.rs --var provider=$P --var ledger=$LED --signer-index $PI --live

# 3. provider releases the rest of the escrow back to the buyer
scripts/submit.py demo/04-release-lane.rs --var provider=$P --var ledger=$LED --var lane_id=<lane id> --signer-index $PI --live
```

Each call costs roughly 0.003–0.005 tTARI in fees. [`demo/README.md`](demo/README.md) records the
first v0.2 run, with transaction ids. [Ootlebook](https://github.com/alex3000-ai/ootlebook)'s service
board lists services by directory agents and shows their lanes as receipts.

## Build and test

Needs rustup's toolchain with the `wasm32-unknown-unknown` target first on `PATH`; the test harness
shells out to `cargo` to compile each template. The tests run the ledger against a real
`AgentDirectory`, built from a sibling checkout of
[ootle-agent-directory](https://github.com/alex3000-ai/ootle-agent-directory) (`../ootle-agent-directory`).

```bash
git clone https://github.com/alex3000-ai/ootle-agent-directory ../ootle-agent-directory
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --target wasm32-unknown-unknown -p lanes_ledger
cargo test -p lanes_tests      # in-process Ootle engine, no network needed
```

`tari_crypto` is pinned to `=0.24.0` because `tari_template_test_tooling` 0.47.0 doesn't compile
against 0.24.1.

Deploying your own: `node deploy/publish-ledger.mjs --account <account>` (dry run; `--live` publishes,
about 2.1 tTARI on Esmeralda), then `deploy/01-ledger.rs` with `scripts/submit.py`.

## Known limits

- State lives in maps on the component, so every call loads all lanes and listings. Fine for a
  testnet; per-entry substates come once real usage shows the sizes.
- No disputes: a lane is only as good as the vouchers the buyer chooses to sign. Buyers should
  sign after each delivered piece of work, so the most a provider can lose is one unpaid piece and
  the most a buyer can lose is one prepaid piece.
- No buyer policies: v0.1's human-verified buyers needed admin-appointed attesters, and v0.2 has no
  admin. A provider that wants to vet buyers can do it off-chain before doing work.

## Origin

Extracted from the payment-lane design in the OotleMayhem fork work (a port of OpenMayhem's
inference marketplace to Ootle), generalised for any agent service.

## Contributing

Issues and pull requests are welcome, especially from people building agent services or clients
in other languages.

- `cargo test -p lanes_tests` must pass, and new ledger behaviour needs a test that checks the
  accounting invariant above.
- A change to the voucher format or signature scheme breaks every client. Bump the domain
  (`…-v2`) rather than changing v1 in place.
- Never commit voucher keys or wallet secrets. `.deploy/` and `*.key` are gitignored.
- Security problems: please open a private advisory on GitHub instead of a public issue.
  Template bugs that let funds move without a valid voucher are the most important kind.

## License

[MIT](LICENSE)
