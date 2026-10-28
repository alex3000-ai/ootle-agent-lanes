#!/usr/bin/env node
// Ootle Agent Lanes v0.3: every lane call, plus reads from the public indexer.
//
//   node lanes.mjs config                                          ledger settings and lane count
//   node lanes.mjs lane <id>                                       one lane
//   node lanes.mjs lanes [--provider N] [--buyer KEY] [--limit 50] newest lanes first
//   node lanes.mjs open --account A <provider> <microtari> <voucher public key hex> [--live]
//   node lanes.mjs fund --account A <lane id> <microtari> [--live]
//   node lanes.mjs claim --account A <voucher.json> [--live]      provider; the voucher from voucher.mjs sign
//   node lanes.mjs close --account A <lane id> [--live]            buyer: start the close window
//   node lanes.mjs release --account A <lane id> [--live]          provider: close at once
//   node lanes.mjs refund --account A <lane id> [--live]           buyer: take back what wasn't claimed
//
// Lanes are NFTs that walletd can't detect, so every call goes through `transactions.submit` with the lane (and,
// for open, the provider's directory card) declared as inputs. Writes dry-run unless --live; --max-fee sets the
// fee ceiling (default 15000 µT). Environment: WALLETD_URL (default http://127.0.0.1:5100/json_rpc),
// WALLETD_TOKEN or WALLETD_TOKEN_FILE, OOTLE_INDEXER, LANES_LEDGER.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { ristretto255 } from "@noble/curves/ed25519.js";
import { callMethod, callMethodWs, cbor, submitInstructions, toWorkspace } from "./tx.mjs";
import { verifyVoucher } from "./voucher.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const TARI = "resource_" + "01".repeat(32);

export function deployment() {
  const path = join(HERE, "..", "deployments", "esmeralda.json");
  const file = existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : {};
  return {
    ledger: process.env.LANES_LEDGER || file.components?.ledger,
    directory: file.directory?.component,
    cards: file.directory?.cards_resource,
    indexer: (process.env.OOTLE_INDEXER || file.indexer || "https://ootle-indexer-a.tari.com").replace(/\/+$/, ""),
  };
}

// ---- reads ----

async function indexer(path) {
  const res = await fetch(`${deployment().indexer}${path}`, { headers: { accept: "application/json" } });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`indexer returned ${res.status} for ${path.slice(0, 60)}`);
  return res.json();
}

const bytesHex = (v) => (v?.["@cbor"] === "bytes" ? v.hex : v?.["@cbor"] === "tag" ? bytesHex(v.value) : null);
const num = (v) => Number(v?.["@cbor"] === "tag" ? v.value : v);

/** Ledger state: [directory, lane_close_epochs, escrow, lanes resource, pages, count]. */
export async function readLedger(address = deployment().ledger) {
  const got = await indexer(`/substates/${address}`);
  const state = got?.state?.up?.substate?.Component?.body?.state;
  if (!Array.isArray(state) || state.length !== 6) throw new Error(`${address} is not a v0.3 ledger`);
  const [dir, closeEpochs, , lanes, , count] = state;
  return { address, directory: `component_${bytesHex(dir)}`, lane_close_epochs: num(closeEpochs), lanes: `resource_${bytesHex(lanes)}`, count: num(count) };
}

export const laneSubstate = (ledger, id) => `nft_${ledger.lanes.replace("resource_", "")}_u64_${id}`;

/** One lane: LaneInfo [id, buyer, provider, provider_key, voucher_key, opened_epoch] and LaneState [deposited, claimed,
 *  refunded, close_requested_epoch, closed]. `remaining` is what the lane still holds. */
export async function readLane(ledger, id) {
  const nft = (await indexer(`/substates/${laneSubstate(ledger, id)}`))?.state?.up?.substate?.NonFungible;
  if (!nft) return null;
  const [, buyer, provider, providerKey, voucherKey, opened] = nft.data;
  const [deposited, claimed, refunded, closeRequested, closed] = nft.mutable_data;
  return {
    lane_id: id, buyer: bytesHex(buyer), provider, provider_key: bytesHex(providerKey), voucher_key: bytesHex(voucherKey),
    opened_epoch: num(opened), deposited: num(deposited), claimed: num(claimed), refunded: num(refunded),
    remaining: num(deposited) - num(claimed) - num(refunded),
    close_requested_epoch: closeRequested === null ? null : num(closeRequested), closed: closed === true,
  };
}

export async function listLanes({ provider, buyer, limit = 50 } = {}) {
  const ledger = await readLedger();
  const out = [];
  for (let id = ledger.count; id >= 1 && out.length < limit; id--) {
    const lane = await readLane(ledger, id);
    if (lane && (!provider || lane.provider === provider) && (!buyer || lane.buyer === buyer)) out.push(lane);
  }
  return out;
}

// ---- writes ----

function walletd() {
  const url = process.env.WALLETD_URL || "http://127.0.0.1:5100/json_rpc";
  let token = process.env.WALLETD_TOKEN ?? (process.env.WALLETD_TOKEN_FILE ? readFileSync(process.env.WALLETD_TOKEN_FILE, "utf8").trim() : undefined);
  let id = 0;
  const rpc = async (method, params) => {
    // No token given: ask a `--authentication none` daemon for a short-lived one.
    if (!token && method !== "auth.request") token = (await rpc("auth.request", { permissions: ["Admin"], credentials: "None" })).token;
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

async function accountInfo(rpc, name) {
  const { accounts } = await rpc("accounts.list", { offset: 0, limit: 100 });
  const a = accounts.find((x) => x.account.name === name);
  if (!a) throw new Error(`no walletd account named "${name}"`);
  const index = a.account.owner_key_id?.Derived?.index;
  if (index === undefined) throw new Error(`account "${name}" has no derived key index`);
  return { component: a.account.component_address, key: a.account.owner_public_key, index };
}

export const validName = (name) => /^[a-z0-9][a-z0-9_-]{1,31}$/.test(name);
export const cardSubstate = (name) => `nft_${deployment().cards.replace("resource_", "")}_str_${name}`;
const hex32 = (h, what) => {
  if (!/^[0-9a-f]{64}$/i.test(h ?? "")) throw new Error(`${what} must be 32 bytes of hex`);
  return [...Buffer.from(h, "hex")];
};
const voucherKey = (h) => {
  const bytes = hex32(h, "voucher key");
  // The ledger accepts any 32 bytes, but no voucher can be signed for a key that isn't a curve point: the lane
  // could then only be closed and refunded.
  try { ristretto255.Point.fromBytes(Uint8Array.from(bytes)); } catch { throw new Error("voucher key is not a valid Ristretto public key"); }
  return bytes;
};
const microtari = (v) => {
  if (!/^[1-9][0-9]*$/.test(String(v))) throw new Error("amount must be a positive whole number of microtari");
  return BigInt(v);
};
/** Withdraws `amount` of tTARI from the account into workspace 0. */
const withdrawTo0 = (account, amount) => [callMethod(account, "withdraw", [cbor.tag(131, cbor.bytes(Buffer.from(TARI.replace("resource_", ""), "hex"))), cbor.uint(amount)]), toWorkspace(0)];
/** Deposits the bucket the previous instruction returned into the account. */
const depositBack = (account) => [toWorkspace(1), callMethodWs(account, "deposit", [{ ws: 1 }])];

/** `name`'s card as the indexer has it: { agent_key, status, channel } or null. Card state is [agent_key, owner_key,
 *  messaging_address, description, channel, status, nonce, updated_epoch]. */
export async function readCard(name) {
  const card = (await indexer(`/substates/${cardSubstate(name)}`))?.state?.up?.substate?.NonFungible?.mutable_data;
  if (!Array.isArray(card)) return null;
  const channel = bytesHex(card[4]);
  return { agent_key: bytesHex(card[0]), status: card[5] === 0 ? "active" : "retired", channel: channel ? `component_${channel}` : null };
}

async function send(flags, build) {
  const rpc = walletd();
  const acct = await accountInfo(rpc, flags.account || "agent");
  const { instructions, inputs } = await build(acct);
  return submitInstructions(rpc, {
    feeAccount: acct.component, signerIndex: acct.index, instructions, inputs, indexer: deployment().indexer,
    live: flags.live === true, ceiling: Number(flags["max-fee"] ?? 15_000),
  });
}

/** Loads a lane and checks the signing account is the one allowed to act on it (`role` "buyer" or "provider"). */
async function ownLane(acct, laneId, role) {
  const ledger = await readLedger();
  const lane = await readLane(ledger, laneId);
  if (!lane) throw new Error(`no lane ${laneId}`);
  const key = role === "buyer" ? lane.buyer : lane.provider_key;
  if (acct.key && acct.key !== key) throw new Error(`this account isn't lane ${laneId}'s ${role} (${key.slice(0, 12)}…)`);
  return { ledger, lane, input: { id: laneSubstate(ledger, laneId), write: true } };
}

export const openLane = (flags, provider, amount, key) =>
  send(flags, async (acct) => {
    if (!validName(provider)) throw new Error(`invalid directory name "${provider}"`);
    const card = await readCard(provider);
    if (!card) throw new Error(`"${provider}" is not in the Ootle Agent Directory`);
    if (card.status !== "active") throw new Error(`"${provider}" is retired in the directory`);
    if (acct.key === card.agent_key) throw new Error("cannot open a lane to yourself");
    return {
      instructions: [...withdrawTo0(acct.component, microtari(amount)), callMethodWs(deployment().ledger, "open_lane", [cbor.text(provider), cbor.bytes(voucherKey(key)), { ws: 0 }])],
      // Reading a card cross-template loads the card and the channel it points at.
      inputs: [cardSubstate(provider), ...(card.channel ? [card.channel] : [])],
    };
  });

export const fundLane = (flags, laneId, amount) =>
  send(flags, async (acct) => {
    const { lane, input } = await ownLane(acct, laneId, "buyer");
    if (lane.closed || lane.close_requested_epoch !== null) throw new Error(`lane ${laneId} is closing or closed`);
    return { instructions: [...withdrawTo0(acct.component, microtari(amount)), callMethodWs(deployment().ledger, "fund_lane", [cbor.uint(laneId), { ws: 0 }])], inputs: [input] };
  });

export const claim = (flags, voucher) =>
  send(flags, async (acct) => {
    const laneId = Number(voucher.lane_id);
    const { ledger, lane, input } = await ownLane(acct, laneId, "provider");
    if (voucher.ledger !== ledger.address) throw new Error(`the voucher is for ${voucher.ledger}, not this ledger`);
    if (voucher.public_key !== lane.voucher_key) throw new Error(`the voucher isn't signed by lane ${laneId}'s voucher key`);
    if (!verifyVoucher(voucher)) throw new Error("the voucher's signature is invalid");
    if (lane.closed) throw new Error(`lane ${laneId} is closed`);
    const spent = BigInt(voucher.spent_cum);
    if (spent <= BigInt(lane.claimed)) throw new Error(`the voucher (${spent}) doesn't exceed what's claimed (${lane.claimed})`);
    if (spent > BigInt(lane.deposited)) throw new Error(`the voucher (${spent}) exceeds the lane's escrow (${lane.deposited})`);
    // SignaturePayload::RistrettoSchnorrBlake2b(signature): [variant 0, [[public nonce, s]]].
    const signature = cbor.array([cbor.uint(0), cbor.array([cbor.array([cbor.bytes(hex32(voucher.public_nonce, "public nonce")), cbor.bytes(hex32(voucher.signature, "signature"))])])]);
    return { instructions: [callMethod(ledger.address, "claim", [cbor.uint(laneId), cbor.uint(spent), signature]), ...depositBack(acct.component)], inputs: [input] };
  });

export const requestClose = (flags, laneId) =>
  send(flags, async (acct) => {
    const { ledger, lane, input } = await ownLane(acct, laneId, "buyer");
    if (lane.closed || lane.close_requested_epoch !== null) throw new Error(`lane ${laneId} is already closing or closed`);
    return { instructions: [callMethod(ledger.address, "request_close", [cbor.uint(laneId)])], inputs: [input] };
  });

export const releaseLane = (flags, laneId) =>
  send(flags, async (acct) => {
    const { ledger, lane, input } = await ownLane(acct, laneId, "provider");
    if (lane.closed) throw new Error(`lane ${laneId} is closed`);
    return { instructions: [callMethod(ledger.address, "release_lane", [cbor.uint(laneId)])], inputs: [input] };
  });

export const refund = (flags, laneId) =>
  send(flags, async (acct) => {
    const { ledger, lane, input } = await ownLane(acct, laneId, "buyer");
    if (lane.remaining <= 0) throw new Error(`lane ${laneId} has nothing to refund`);
    if (!lane.closed) {
      if (lane.close_requested_epoch === null) throw new Error(`close lane ${laneId} first`);
      const stats = await indexer("/epoch-manager/stats");
      const ready = lane.close_requested_epoch + ledger.lane_close_epochs;
      if (Number(stats.current_epoch) < ready) throw new Error(`lane ${laneId} can be refunded from epoch ${ready} (now ${stats.current_epoch})`);
    }
    return { instructions: [callMethod(ledger.address, "refund", [cbor.uint(laneId)]), ...depositBack(acct.component)], inputs: [input] };
  });

async function main(argv) {
  const [cmd, ...rest] = argv;
  const flags = {}, args = [];
  for (let i = 0; i < rest.length; i++) {
    if (rest[i].startsWith("--")) flags[rest[i].slice(2)] = rest[i + 1] !== undefined && !rest[i + 1].startsWith("--") ? rest[++i] : true;
    else args.push(rest[i]);
  }
  const done = (r, what) => console.log(r.dry_run ? `Dry run accepted (${what}): fee ${r.required_fee} µT. Add --live to send.` : `${what}: ${r.transaction_id} (fee ${r.fee} µT)`);
  const lane = () => {
    const id = Number(args[0]);
    if (!Number.isSafeInteger(id) || id < 1) throw new Error("lane id must be a positive integer");
    return id;
  };
  const need = (n, usage) => { if (args.length !== n) throw new Error(`usage: ${usage}`); };
  switch (cmd) {
    case "config": console.log(JSON.stringify(await readLedger(), null, 2)); break;
    case "lane": console.log(JSON.stringify(await readLane(await readLedger(), lane()), null, 2)); break;
    case "lanes": console.log(JSON.stringify(await listLanes({ provider: flags.provider, buyer: flags.buyer, limit: Number(flags.limit ?? 50) }), null, 2)); break;
    case "open": {
      need(3, "open --account A <provider> <microtari> <voucher public key hex> [--live]");
      const r = await openLane(flags, args[0], args[1], args[2]);
      const opened = JSON.stringify(r.result ?? {}).match(/"lane_id":"?(\d+)/)?.[1];
      done(r, `lane ${opened ?? ""} to ${args[0]} opened`.replace("  ", " "));
      break;
    }
    case "fund": need(2, "fund --account A <lane id> <microtari> [--live]"); done(await fundLane(flags, lane(), args[1]), `lane ${args[0]} funded`); break;
    case "claim": {
      need(1, "claim --account A <voucher.json> [--live]");
      const voucher = JSON.parse(readFileSync(args[0], "utf8"));
      done(await claim(flags, voucher), `claimed ${voucher.spent_cum} µT (cumulative) on lane ${voucher.lane_id}`);
      break;
    }
    case "close": need(1, "close --account A <lane id> [--live]"); done(await requestClose(flags, lane()), `close of lane ${args[0]} requested`); break;
    case "release": need(1, "release --account A <lane id> [--live]"); done(await releaseLane(flags, lane()), `lane ${args[0]} released`); break;
    case "refund": need(1, "refund --account A <lane id> [--live]"); done(await refund(flags, lane()), `lane ${args[0]} refunded`); break;
    default:
      console.log(readFileSync(fileURLToPath(import.meta.url), "utf8").split("\n").slice(1, 13).map((l) => l.replace(/^\/\/ ?/, "")).join("\n"));
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).catch((e) => { console.error(e.message); process.exit(1); });
}
