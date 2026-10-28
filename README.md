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

**Status: v0.3, live on the Esmeralda testnet** (addresses in [`deployments/esmeralda.json`](deployments/esmeralda.json),
a full lane run in [`demo/`](demo/README.md)). The template is not audited. Don't put anything of value in it.

## No admin

`LaneLedger` takes its settings once, when it is created: the directory, the payment resource and the close window
(6 epochs on Esmeralda). There is no owner, no fee, no pause and no method that changes a setting, so nobody can alter
the terms of an open lane. Check it yourself: the component's `owner_rule` is `None` and its access rules allow every
method to everyone; each method checks the transaction signer itself.

## How a lane works

1. The buyer calls `open_lane(provider, voucher_key, funds)`, where `provider` is a directory name and `funds` a
   bucket of tTARI. The ledger reads the card: the agent must be active, and the lane records the card's agent key at
   that moment. `voucher_key` is the key that will sign the lane's vouchers (see below). `fund_lane` adds more.
2. For each piece of work, the buyer signs a voucher for the **cumulative** amount spent so far and hands it to the
   provider off-chain.
3. The provider calls `claim(lane_id, spent_cum, signature)` from the lane's key and receives `spent_cum - claimed`,
   in full.
4. To stop, the buyer calls `request_close`. The provider can still claim until the close window has passed; then
   the buyer calls `refund` and gets back what wasn't claimed. The provider can also `release_lane` at once, which
   lets the buyer refund straight away.

A lane stays with the key it was opened to. If the directory owner rotates the provider's agent key, old lanes are
claimed with the old key and new lanes go to the new one. A retired provider can't get new lanes; its open lanes still
settle.

**Fees stay flat.** Each lane is a non-fungible token of the ledger's own resource (`nft_<lanes resource>_u64_<id>`,
ids 1, 2, 3…) kept in page vaults of 256, so a call loads one lane, not all of them. Escrow sits in one vault:
`escrow == sum over lanes of (deposited - claimed - refunded)`, checked by the tests after every step. On Esmeralda
lane 22 cost 85 µT more to open than lane 1 (`demo/`).

### Voucher keys

Vouchers are signed by the lane's `voucher_key`, not by the transaction that opened it. It can be the buyer's own key,
but usually it's a hot key the agent holds in a file: Ootle wallet daemons can't sign arbitrary messages, and a leaked
voucher key can spend at most that one lane's escrow.

### Voucher format

Clients must reproduce this exactly:

- domain: `ootle-agent-lane-voucher-v1`
- message: UTF-8 `"<ledger component address>|<lane id>|<spent_cum as integer>"` (`lanes_ledger::lane_voucher_message`).
  Binding the ledger address stops a voucher being replayed against another deployment.
- signature: Ristretto Schnorr with challenge `Blake2b-512(domain ‖ public nonce ‖ public key ‖ message)`, as in
  `tari_ootle_common_types::RistrettoSchnorrBlake2bVerifier`.

## Client

Node 20+, in `client/` (`npm install`; depends only on `@noble/curves` and `@noble/hashes`). Lanes are NFTs that
wallet daemons don't detect from a manifest, so `lanes.mjs` sends every call as a raw transaction with the lane (and,
for `open`, the provider's card) declared. Every write dry-runs unless `--live`; the account needs a little revealed
tTARI for fees.

```bash
export WALLETD_URL=http://127.0.0.1:<port>/json_rpc        # a local tari_ootle_walletd 0.47 (--authentication none, or WALLETD_TOKEN)
node voucher.mjs keygen --out voucher.key                   # buyer: prints the voucher public key
node lanes.mjs open --account buyer someagent 500000 <voucher public key> --live
node voucher.mjs sign --key-file voucher.key <ledger> <lane id> 200000 > v.json   # buyer, per piece of work
node voucher.mjs verify "$(cat v.json)"                     # provider: check before doing the work
node lanes.mjs claim --account provider v.json --live
node lanes.mjs close --account buyer <lane id> --live       # then, after the window: refund
node lanes.mjs release --account provider <lane id> --live  # or the provider closes at once
node lanes.mjs refund --account buyer <lane id> --live
node lanes.mjs lanes --provider someagent ; node lanes.mjs lane 1 ; node lanes.mjs config
```

Each call costs about 0.003–0.006 tTARI. `npm test` (in `client/`) checks vouchers against the template's format.

## Earlier versions

v0.2 kept every lane and buyer balance in maps on the component, so every call got dearer with every lane ever opened
(about 155 µT per lane; 24,225 µT for `open_lane` at 128 lanes). v0.1 had an admin badge. Both are frozen: no new
lanes should open there, and anyone with funds in them can still take them out with the manifests in `legacy/`
(`scripts/submit.py legacy/v0.2-withdraw.rs --var buyer=<account> --var ledger=<v0.2 ledger> --var amount=<µT>
--signer-index <i> [--live]`). Their addresses are under `v0_2` and `superseded` in `deployments/esmeralda.json`.

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

Deploying your own: `node deploy/publish-ledger.mjs --account <account>` dry-runs publishing and creating the ledger;
`--live` does both (about 2.6 tTARI on Esmeralda). The release profile builds with `opt-level = "z"`: publishing is
charged by code size, steeply (the same template built with `"s"` was quoted 3.14 tTARI).

## Known limits

- No disputes: a lane is only as good as the vouchers the buyer chooses to sign. Buyers should
  sign after each delivered piece of work, so the most a provider can lose is one unpaid piece and
  the most a buyer can lose is one prepaid piece.
- No buyer policies: v0.1's human-verified buyers needed admin-appointed attesters, and later versions have no
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
