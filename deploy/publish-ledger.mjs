#!/usr/bin/env node
// Publishes the LaneLedger template and creates the ledger over the official directory, with a 6-epoch close window.
// Dry-runs by default; --live spends and records both in deployments/esmeralda.json. Nothing can change the settings
// later and the ledger has no owner. Build first:
//   PATH="$HOME/.cargo/bin:$PATH" cargo build --release --target wasm32-unknown-unknown -p lanes_ledger
//   node deploy/publish-ledger.mjs --account lanes-admin [--live]
//   node deploy/publish-ledger.mjs --account lanes-admin --create-only [--live]   (template already published)

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { accountInfo, callFunction, cbor, parseArgs, publishTemplate, submitInstructions, walletd } from "../client/tx.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const WASM = join(ROOT, "target/wasm32-unknown-unknown/release/lanes_ledger.wasm");
const PATH = join(ROOT, "deployments", "esmeralda.json");
const CLOSE_EPOCHS = 6;
const { flags } = parseArgs(process.argv.slice(2));
const live = flags.live === true;
const account = flags.account || "lanes-admin";
const rpc = walletd();
const dep = JSON.parse(readFileSync(PATH, "utf8"));
const save = () => writeFileSync(PATH, JSON.stringify(dep, null, 2) + "\n");

if (!flags["create-only"]) {
  const p = await publishTemplate(rpc, { wasm: WASM, account, live });
  if (!live) {
    console.log(`publish: dry run fee ${p.dry_run_fee} µT for a ${(readFileSync(WASM).length / 1024).toFixed(0)} KiB template`);
    process.exit(0);
  }
  dep.templates.LaneLedger = p.template;
  dep.transactions.publish_ledger = p.transaction_id;
  save();
  console.log(`published ${p.template} in ${p.transaction_id} (fee ${p.fee} µT)`);
}

// new(directory, payment_resource, lane_close_epochs)
const acct = await accountInfo(rpc, account);
const r = await submitInstructions(rpc, {
  feeAccount: acct.component, signerIndex: acct.index, live, ceiling: Number(flags["max-fee"] ?? 20_000),
  instructions: [callFunction(dep.templates.LaneLedger, "new", [cbor.component(dep.directory.component), cbor.resource(dep.payment_resource), cbor.uint(CLOSE_EPOCHS)])],
});
if (!live) {
  console.log(`create: dry run fee ${r.required_fee} µT`);
  process.exit(0);
}
const text = JSON.stringify(r.result);
dep.components.ledger = text.match(/component_[0-9a-f]{64}/g)?.find((c) => c !== dep.directory.component && c !== acct.component);
dep.lanes_resource = text.match(/resource_[0-9a-f]{64}/g)?.find((x) => x !== dep.payment_resource);
dep.transactions.create_ledger = r.transaction_id;
save();
console.log(`created ledger ${dep.components.ledger} (lanes ${dep.lanes_resource}) in ${r.transaction_id} (fee ${r.fee} µT); recorded in deployments/esmeralda.json`);
