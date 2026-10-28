// Copied from github.com/alex3000-ai/ootle-agent-directory (client/tx.mjs); keep the two in step.
// Raw Ootle transactions through walletd `transactions.submit`, for calls whose inputs walletd can't detect
// from a manifest: a channel reads its agent's card through the directory, so the card must be declared.
// Arguments are CBOR, built here, so text needs no escaping.

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
  const text = JSON.stringify(dry);
  const failure = text.match(/"ExecutionFailure":\s*(\{[^}]*\})/)?.[1] ?? text.match(/"Reject":\s*("[^"]*"|\{[^}]*\})/)?.[1];
  if (failure || /"(Reject|AcceptFeeRejectRest)"\s*:/.test(text)) throw new Error(`dry run rejected: ${(failure ?? "the transaction would fail").slice(0, 400)}`);
  const fee = Number(dry.required_fees ?? ceiling);
  // Dry runs have come in up to ~12% under the fee the network then required, so leave room.
  const maxFee = Math.min(ceiling, Math.ceil(fee * 1.25) + 500);
  if (!live) return { dry_run: true, required_fee: fee };
  const sent = await rpc("transactions.submit", req(maxFee, false));
  const result = await rpc("transactions.wait_result", { transaction_id: sent.transaction_id, timeout_secs: 240 });
  if (result.status !== "Accepted") throw new Error(`transaction ${sent.transaction_id}: ${result.status} ${JSON.stringify(result).slice(0, 600)}`);
  return { transaction_id: sent.transaction_id, fee: result.final_fee, result };
}
