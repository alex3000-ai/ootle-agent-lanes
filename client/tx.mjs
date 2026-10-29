// Copied from github.com/alex3000-ai/ootle-agent-directory (client/tx.mjs); keep the two in step.
// Raw Ootle transactions through walletd `transactions.submit`, for calls whose inputs walletd can't detect
// from a manifest: a channel reads its agent's card through the directory, so the card must be declared.
// Arguments are CBOR, built here, so text needs no escaping. Also the walletd connection, template publishing
// and the small command-line helpers every client here shares.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export const TARI = "resource_" + "01".repeat(32);
const hex = (b) => Buffer.from(b).toString("hex");

function head(major, n) {
  if (n < 24) return [(major << 5) | n];
  if (n < 0x100) return [(major << 5) | 24, n];
  if (n < 0x10000) return [(major << 5) | 25, n >> 8, n & 0xff];
  if (n < 0x100000000) return [(major << 5) | 26, (n >>> 24) & 0xff, (n >>> 16) & 0xff, (n >>> 8) & 0xff, n & 0xff];
  const b = Buffer.alloc(9);
  b[0] = (major << 5) | 27;
  b.writeBigUInt64BE(BigInt(n), 1);
  return [...b];
}

/** CBOR values for instruction arguments. */
export const cbor = {
  uint: (n) => {
    const v = BigInt(n);
    if (v < 0n || v > 0xffffffffffffffffn) throw new Error("uint out of range");
    return v > BigInt(Number.MAX_SAFE_INTEGER) ? (() => { const b = Buffer.alloc(9); b[0] = 27; b.writeBigUInt64BE(v, 1); return [...b]; })() : head(0, Number(v));
  },
  text: (s) => { const b = Buffer.from(String(s), "utf8"); return [...head(3, b.length), ...b]; },
  bytes: (b) => [...head(2, b.length), ...b],
  array: (items) => [...head(4, items.length), ...items.flat()],
  null: () => [0xf6],
  tag: (t, v) => [...head(6, t), ...v],
  component: (addr) => {
    const m = /^component_([0-9a-f]{64})$/.exec(addr);
    if (!m) throw new Error(`not a component address: ${addr}`);
    return cbor.tag(128, cbor.bytes(Buffer.from(m[1], "hex")));
  },
  resource: (addr) => {
    const m = /^resource_([0-9a-f]{64})$/.exec(addr);
    if (!m) throw new Error(`not a resource address: ${addr}`);
    return cbor.tag(131, cbor.bytes(Buffer.from(m[1], "hex")));
  },
  /** A 32-byte key given as hex. */
  key: (h, what = "key") => {
    if (!/^[0-9a-f]{64}$/i.test(h ?? "")) throw new Error(`${what} must be 32 bytes of hex`);
    return cbor.bytes([...Buffer.from(h, "hex")]);
  },
  /** SignaturePayload::RistrettoSchnorrBlake2b: [variant 0, [[public nonce, s]]]. */
  signature: ({ public_nonce, signature }) => cbor.array([cbor.uint(0), cbor.array([cbor.array([cbor.key(public_nonce, "public nonce"), cbor.key(signature, "signature")])])]),
};
const lit = (bytes) => ({ Literal: hex(bytes) });

export const callMethod = (component, method, args) => ({ CallMethod: { call: { Address: component }, method, args: args.map(lit) } });
export const callFunction = (template, fn, args) => ({ CallFunction: { address: template.replace("template_", ""), function: fn, args: args.map(lit) } });
export const toWorkspace = (key) => ({ PutLastInstructionOutputOnWorkspace: { key } });
export const workspaceArg = (id) => ({ Workspace: { id, offset: null } });
/** callMethod with some arguments taken from the workspace: pass {ws: id} for those. */
export const callMethodWs = (component, method, args) => ({
  CallMethod: { call: { Address: component }, method, args: args.map((a) => (a && a.ws !== undefined ? workspaceArg(a.ws) : lit(a))) },
});
/** Withdraws `amount` µT of TARI from the account into workspace 0, for a call that takes a bucket ({ws: 0}). */
export const withdrawTari = (account, amount) => [callMethod(account, "withdraw", [cbor.resource(TARI), cbor.uint(amount)]), toWorkspace(0)];
/** Deposits the bucket the previous instruction returned into the account. */
export const depositReturned = (account) => [toWorkspace(1), callMethodWs(account, "deposit", [{ ws: 1 }])];

function unsigned({ feeAccount, maxFee, instructions, inputs, dryRun, network, maxEpoch }) {
  return {
    V1: {
      network,
      fee_instructions: [callMethod(feeAccount, "pay_fee", [cbor.uint(maxFee)])],
      instructions,
      // A string is a read-only input; {id, write: true} is one the transaction writes.
      inputs: inputs.map((i) => (typeof i === "string" ? { substate_id: i, is_write: false, version: null } : { substate_id: i.id, is_write: i.write === true, version: null })),
      min_epoch: null,
      max_epoch: maxEpoch,
      blob_hashes: [],
      blob_sizes: [],
      dry_run: dryRun,
      is_seal_signer_authorized: true,
      nonce: Number(BigInt.asUintN(53, BigInt("0x" + hex(crypto.getRandomValues(new Uint8Array(8)))))),
    },
  };
}

/** Why a transaction result would fail, or null: the reason of a Reject or AcceptFeeRejectRest outcome. */
export function rejection(result) {
  let reason = null;
  const walk = (o) => {
    if (reason !== null || !o || typeof o !== "object") return;
    if ("Reject" in o) reason = o.Reject;
    else if (Array.isArray(o.AcceptFeeRejectRest)) reason = o.AcceptFeeRejectRest[1] ?? "rejected";
    else Object.values(o).forEach(walk);
  };
  walk(result);
  if (reason === null) return null;
  const inner = reason && typeof reason === "object" ? Object.values(reason)[0] : reason;
  return (typeof inner === "string" ? inner : inner?.message ?? JSON.stringify(reason)).slice(0, 400);
}

/** The max_fee to send for a dry run's fee: dry runs have come in up to ~12% under what the network then charged. */
export const feeWithMargin = (fee) => Math.ceil(fee * 1.25) + 500;

/**
 * Dry-runs, then (with live) submits with max_fee set from the dry run. The account needs `ceiling` µT of revealed
 * tTARI on hand for the dry run, so keep it near a few times a call's real cost (calls run about 3,000–7,000 µT). `inputs` are substates walletd must
 * declare beyond those it detects. Declare writes correctly: a dry run accepts a write to a read-only input,
 * but the network rejects it (and still charges the fee). Returns {dry_run, required_fee} or {transaction_id, fee, result}.
 */
export async function submitInstructions(rpc, { feeAccount, signerIndex, instructions, inputs = [], live, ceiling = 15_000, network = 0x26, indexer = "https://ootle-indexer-a.tari.com" }) {
  // Transactions expire: allow a few epochs from now, as walletd does for manifests.
  const stats = await (await fetch(`${indexer}/epoch-manager/stats`, { headers: { accept: "application/json" } })).json();
  const maxEpoch = Number(stats.current_epoch) + 3;
  if (!Number.isSafeInteger(maxEpoch)) throw new Error("could not read the current epoch from the indexer");
  const req = (maxFee, dryRun) => ({
    transaction: unsigned({ feeAccount, maxFee, instructions, inputs, dryRun, network, maxEpoch }),
    seal_signer: { Derived: { index: signerIndex, key_branch: "account" } },
    other_signers: [],
    detect_inputs: true,
    detect_inputs_use_unversioned: true,
    lock_ids: [],
  });
  const dry = await rpc("transactions.submit_dry_run", req(ceiling, true));
  // A dry run "succeeds" even when the transaction would fail: the outcome is then Reject, or
  // AcceptFeeRejectRest (the network takes the fee and rejects the rest). Treat both as a rejection.
  const failure = rejection(dry);
  if (failure) throw new Error(`dry run rejected: ${failure}`);
  const fee = Number(dry.required_fees ?? ceiling);
  // A dry run is accepted even when its fee is above the ceiling it ran with; the real transaction would then fail.
  const maxFee = feeWithMargin(fee);
  if (maxFee > ceiling) throw new Error(`this call needs a fee of about ${fee} µT (${maxFee} with margin), above the ${ceiling} µT ceiling: rerun with --max-fee ${maxFee}`);
  if (!live) return { dry_run: true, required_fee: fee };
  const sent = await rpc("transactions.submit", req(maxFee, false));
  const result = await rpc("transactions.wait_result", { transaction_id: sent.transaction_id, timeout_secs: 240 });
  if (result.status !== "Accepted") throw new Error(`transaction ${sent.transaction_id}: ${result.status} ${JSON.stringify(result).slice(0, 600)}`);
  return { transaction_id: sent.transaction_id, fee: result.final_fee, result };
}

// ---- walletd ----

/**
 * JSON-RPC to tari_ootle_walletd at WALLETD_URL (default http://127.0.0.1:5100/json_rpc), with WALLETD_TOKEN or the
 * token in WALLETD_TOKEN_FILE. With neither, it asks the daemon for a short-lived token, which only a daemon run with
 * `--authentication none` grants.
 */
export function walletd() {
  const url = process.env.WALLETD_URL || "http://127.0.0.1:5100/json_rpc";
  let token = process.env.WALLETD_TOKEN ?? (process.env.WALLETD_TOKEN_FILE ? readFileSync(process.env.WALLETD_TOKEN_FILE, "utf8").trim() : undefined);
  let id = 0;
  const rpc = async (method, params) => {
    if (!token && method !== "auth.request") token = (await rpc("auth.request", { permissions: ["Admin"], duration: null, credentials: "None" })).token;
    const res = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) },
      body: JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }),
    });
    const body = await res.json();
    if (body.error) throw new Error(`${method}: ${body.error.message}`);
    return body.result;
  };
  return rpc;
}

/** A walletd account's component, public key and key index (the index seals its transactions). */
export async function accountInfo(rpc, name) {
  const { accounts } = await rpc("accounts.list", { offset: 0, limit: 100 });
  const a = accounts.find((x) => x.account.name === name);
  if (!a) throw new Error(`no walletd account named "${name}"`);
  const index = a.account.owner_key_id?.Derived?.index;
  if (index === undefined) throw new Error(`account "${name}" has no derived key index`);
  return { component: a.account.component_address, key: a.account.owner_public_key, index };
}

/**
 * Publishes a template binary from `account`: a dry run for the fee, then (with live) the real thing. Publishing is
 * charged steeply by code size, so build with opt-level "z". Returns {dry_run_fee} or {template, transaction_id, fee}.
 */
export async function publishTemplate(rpc, { wasm, account, live }) {
  const binary = readFileSync(wasm).toString("base64");
  const publish = (dry_run, max_fee) => rpc("transactions.publish_template", { binary, fee_account: { Name: account }, max_fee, detect_inputs: true, dry_run });
  // A dry run reports the full fee even above its max_fee, so a low ceiling keeps the revealed funds it needs small.
  const dry = await publish(true, 1_000_000);
  const failure = rejection(dry);
  if (failure) throw new Error(`publish dry run rejected: ${failure}`);
  if (!live) return { dry_run_fee: dry.dry_run_fee };
  // walletd charges a publish its actual fee, so the margin over the dry run costs nothing.
  const sent = await publish(false, feeWithMargin(dry.dry_run_fee));
  const result = await rpc("transactions.wait_result", { transaction_id: sent.transaction_id, timeout_secs: 300 });
  if (result.status !== "Accepted") throw new Error(`publish failed: ${JSON.stringify(result).slice(0, 800)}`);
  return { template: JSON.stringify(result).match(/template_[0-9a-f]{64}/)?.[0], transaction_id: sent.transaction_id, fee: result.final_fee };
}

// ---- command line ----

/** `--name value` and bare `--flag` (true) into flags; everything else into args. */
export function parseArgs(argv) {
  const flags = {}, args = [];
  for (let i = 0; i < argv.length; i++) {
    if (argv[i].startsWith("--")) flags[argv[i].slice(2)] = argv[i + 1] !== undefined && !argv[i + 1].startsWith("--") ? argv[++i] : true;
    else args.push(argv[i]);
  }
  return { flags, args };
}

/** Prints the comment block at the top of a script (its usage) and exits: 0 when asked for, 1 for an unknown command. */
export function usage(scriptUrl, cmd) {
  const lines = readFileSync(fileURLToPath(scriptUrl), "utf8").split("\n").slice(1);
  console.log(lines.slice(0, lines.findIndex((l) => !l.startsWith("//"))).map((l) => l.replace(/^\/\/ ?/, "")).join("\n"));
  process.exit(cmd ? 1 : 0);
}

/** Prints a write's outcome: the dry run's fee, or the transaction. */
export const report = (r, what) => console.log(r.dry_run ? `Dry run accepted (${what}): fee ${r.required_fee} µT. Add --live to send.` : `${what}: ${r.transaction_id} (fee ${r.fee} µT)`);
