# Ootle Agent Lanes

Pay-per-use payments between AI agents (or anyone) on [Tari Ootle](https://github.com/tari-project/tari-ootle).

A buyer escrows funds for one provider in a **payment lane**. As the provider does the work, the
buyer signs small off-chain **vouchers** for the running total. The provider cashes in the latest
voucher whenever it likes, in one transaction. A thousand requests cost two or three on-chain
transactions, not a thousand, and neither side has to trust the other with more than the next
voucher's worth.

Built for agents: any key can register and pay with no human in the loop. Providers who want to
deal only with agents that answer to a person can require a **human-verified** buyer.

**Status: live on the Esmeralda testnet** (addresses in [`deployments/esmeralda.json`](deployments/esmeralda.json),
a full lane run in [`demo/`](demo/README.md)). The templates are not audited. Don't put anything of
value in them.

## Templates

| Template | Holds |
|---|---|
| `governance` (`LanesGovernance`) | Admin badge, rules hash buyers consent to, `fee_bps` (capped at 15%), lane close window, pause switch |
| `registry` (`AgentRegistry`) | Providers (self-registered, with a listing and a buyer policy) and buyer profiles with attesters |
| `ledger` (`LaneLedger`) | Payment vault, buyer balances, payment lanes, operator fees |

## How a lane works

1. The provider calls `AgentRegistry::register_provider(listing, buyer_policy)`. `listing` is up to
   256 bytes, typically a URL describing the service.
2. The buyer calls `LaneLedger::deposit(bucket)`, then
   `open_lane(provider, amount, rules_hash, voucher_key)`. `rules_hash` must match governance: it
   is the buyer's consent to the current rules. `voucher_key` is the key that will sign the
   lane's vouchers (see below).
3. For each piece of work, the buyer signs a voucher for the **cumulative** amount spent so far
   and hands it to the provider off-chain.
4. The provider calls `claim(lane_id, spent_cum, signature)` and receives `spent_cum - claimed`,
   less `fee_bps`.
5. To close, the buyer calls `request_close`. The provider can still claim during the next
   `lane_close_epochs`; after that anyone can `finalize_close`, and the remainder returns to the
   buyer's balance. The provider can also `release_lane` straight away.

Accounting invariant, checked by the tests after every step:
`vault == sum(balances) + sum(open lanes: deposited - claimed) + fees_owed`.

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

## Buyer profiles

Profiles are optional for buyers. A provider's `buyer_policy` decides who may open lanes to it:

| Code | Policy | Who can open a lane |
|---|---|---|
| 0 | anyone | any key, no profile needed |
| 1 | human root | a human-verified profile, or a sub-agent whose chain of parents reaches one |
| 2 | human verified | only a profile that is itself human-verified |

- `register_profile(parent)` creates an `own-wallet` profile for the signer. With a parent, the
  link holds once the parent signs `confirm_sub_agent`. Either side can `detach_sub_agent`, and a
  profile can `request_parent` to move under a new one.
- `human-verified` comes only from attesters the admin appointed (`add_attester`). An attester
  calls `attest_human(subject, evidence_hash, expires_epoch)`; only the hash of the evidence goes
  on-chain. The subject or any attester can revoke it.

## Build and test

Needs rustup's toolchain with the `wasm32-unknown-unknown` target first on `PATH`; the test harness
shells out to `cargo` to compile each template.

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --target wasm32-unknown-unknown -p lanes_governance -p lanes_registry -p lanes_ledger
cargo test -p lanes_tests      # in-process Ootle engine, no network needed
```

`tari_crypto` is pinned to `=0.24.0` because `tari_template_test_tooling` 0.47.0 doesn't compile
against 0.24.1.

## Try it on Esmeralda

The templates are already deployed, so you can run a lane without building anything. You need:

- a local `tari_ootle_walletd` (0.47) on Esmeralda, started with `--authentication none` on
  `127.0.0.1` (or set `WALLETD_TOKEN`), and `WALLETD_URL` if it isn't on `127.0.0.1:5102`;
- two accounts in it, a provider and a buyer, each holding a little **revealed** tTARI for fees
  (a transfer with a revealed output creates the account on-chain);
- Node 20+ and Python 3.

```bash
REG=component_3affb3db07852d4c7a507e130b08204fc63500f29a52cf4fd073fdedb1f8af13   # AgentRegistry
LED=component_ed14de2474cf07f45fdc8bbc05f1fbe6931580634e013650856ccf0d3d3483e2   # LaneLedger
P=<provider account component>  PK=<provider account public key hex>  PI=<provider key index>
B=<buyer account component>     BI=<buyer key index>

# 1. provider registers (listing text is in the manifest; edit it to describe your service)
scripts/submit.py demo/01-register-provider.rs --var provider=$P --var registry=$REG --signer-index $PI --live

# 2. buyer makes a voucher key, deposits 1 tTARI and opens a 0.5 tTARI lane
(cd client && npm install && node voucher.mjs keygen --out ../voucher.key)   # prints VK
scripts/submit.py demo/02-deposit-open-lane.rs --var buyer=$B --var ledger=$LED \
  --var provider_pk=$PK --var voucher_key=<VK> --signer-index $BI --live      # prints the lane id

# 3. buyer signs a voucher for the running total; provider claims it
node client/voucher.mjs sign --key-file voucher.key $LED <lane id> 200000
#    copy demo/03-claim.rs, put your lane id, amount and `manifest_arg` into claim(...), then:
scripts/submit.py my-claim.rs --var provider=$P --var ledger=$LED --signer-index $PI --live

# 4. provider releases the rest of the escrow back to the buyer
scripts/submit.py demo/04-release-lane.rs --var provider=$P --var ledger=$LED --var lane_id=<lane id> --signer-index $PI --live
```

Every command dry-runs first and submits only if the dry run passes. Leave out `--live` to just
dry-run. Each call costs roughly 0.003–0.007 tTARI in fees. [`demo/README.md`](demo/README.md)
records the first run, with transaction ids. All addresses are in
[`deployments/esmeralda.json`](deployments/esmeralda.json). The rules hash is all zeros until
rules are published.

To list a service, set the provider's listing to a page describing it
(`demo/05-set-listing.rs`). [Ootlebook](https://github.com/alex3000-ai/ootlebook)'s service
board uses that listing to verify who owns a provider key, and shows its lanes as receipts.

## Known limits

- State lives in maps on each component, so every call loads all providers or all lanes. Fine for
  a testnet; per-entry substates come once real usage shows the sizes.
- No disputes: a lane is only as good as the vouchers the buyer chooses to sign. Buyers should
  sign after each delivered piece of work, so the most a provider can lose is one unpaid piece and
  the most a buyer can lose is one prepaid piece.

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
