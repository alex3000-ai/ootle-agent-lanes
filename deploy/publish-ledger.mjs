#!/usr/bin/env node
// Publishes the LaneLedger template. Dry-runs by default; --live spends and records the template in
// deployments/esmeralda.json. Build first:
//   PATH="$HOME/.cargo/bin:$PATH" cargo build --release --target wasm32-unknown-unknown -p lanes_ledger
//   node deploy/publish-ledger.mjs --account lanes-admin [--live]
// Then create the component with deploy/01-ledger.rs (scripts/submit.py).

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const WASM = join(ROOT, "target/wasm32-unknown-unknown/release/lanes_ledger.wasm");
const live = process.argv.includes("--live");
const account = process.argv[process.argv.indexOf("--account") + 1] || "lanes-admin";
const url = process.env.WALLETD_URL || "http://127.0.0.1:5100/json_rpc";
let token = process.env.WALLETD_TOKEN;
let id = 0;
async function rpc(method, params) {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) },
    body: JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }),
  });
  const body = await res.json();
  if (body.error) throw new Error(`${method}: ${body.error.message}`);
  return body.result;
}
// Publishing needs Templates(Create): ask a `--authentication none` daemon for a short-lived admin token.
if (!token) token = (await rpc("auth.request", { permissions: ["Admin"], credentials: "None" })).token;

const binary = readFileSync(WASM).toString("base64");
const publish = (dry_run, max_fee) => rpc("transactions.publish_template", { binary, fee_account: { Name: account }, max_fee, detect_inputs: true, dry_run });
// A dry run reports the full fee even above its max_fee, so a low ceiling keeps the revealed funds it needs small.
const dry = await publish(true, 1_000_000);
console.log(`dry run fee ${dry.dry_run_fee} µT for a ${(readFileSync(WASM).length / 1024).toFixed(0)} KiB template`);
if (!live) process.exit(0);
// walletd charges the full max_fee, but dry runs have come in up to ~12% low: leave 15%.
const sent = await publish(false, Math.ceil(dry.dry_run_fee * 1.15));
const result = await rpc("transactions.wait_result", { transaction_id: sent.transaction_id, timeout_secs: 300 });
if (result.status !== "Accepted") throw new Error(`publish failed: ${JSON.stringify(result).slice(0, 800)}`);
const template = JSON.stringify(result).match(/template_[0-9a-f]{64}/)?.[0];
const path = join(ROOT, "deployments", "esmeralda.json");
const dep = JSON.parse(readFileSync(path, "utf8"));
dep.templates = { ...dep.templates, LaneLedger: template };
dep.transactions = { ...dep.transactions, publish_ledger: sent.transaction_id };
writeFileSync(path, JSON.stringify(dep, null, 2) + "\n");
console.log(`published ${template} in ${sent.transaction_id} (fee ${result.final_fee} µT); recorded in deployments/esmeralda.json`);
