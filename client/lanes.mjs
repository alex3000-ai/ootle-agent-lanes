#!/usr/bin/env node
// Ootle Agent Lanes: the calls that read a provider's directory card, plus reads from the public indexer.
//
//   node lanes.mjs config                                                ledger settings, as deployed
//   node lanes.mjs listing <name>                                         a provider's listing
//   node lanes.mjs lane <lane id>                                         one lane
//   node lanes.mjs lanes <name>                                           lanes paying a provider
//   node lanes.mjs set-listing --account A --name N "<listing>" [--live]   publish (or "" to remove) N's listing
//   node lanes.mjs open-lane --account A <provider> <microtari> <voucher key hex> [--live]
//
// set-listing and open-lane read the provider's card through the directory, which walletd can't detect, so
// they go through `transactions.submit` with the card declared as an input. Everything else (deposit,
// claim, close, release, withdraw) works as ordinary manifests: see demo/. Writes dry-run unless --live.
// Environment: WALLETD_URL (default http://127.0.0.1:5100/json_rpc), WALLETD_TOKEN or WALLETD_TOKEN_FILE,
// OOTLE_INDEXER, LANES_LEDGER.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { ristretto255 } from "@noble/curves/ed25519.js";
import { callMethod, cbor, submitInstructions } from "./tx.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));

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
// The indexer renders CBOR maps with text keys as JSON objects, and other maps as {"@cbor":"map","entries":[…]}.
const mapEntries = (v) => (v?.["@cbor"] === "map" ? v.entries : Object.entries(v ?? {}));

/** Ledger state: [directory, lane_close_epochs, vault, balances, lanes, next_lane_id, listings]. */
export async function readLedger(address = deployment().ledger) {
  const got = await indexer(`/substates/${address}`);
  const state = got?.state?.up?.substate?.Component?.body?.state;
  if (!Array.isArray(state)) throw new Error(`${address} is not a component`);
  const [dir, closeEpochs, , , lanes, nextLane, listings] = state;
  return {
    directory: `component_${bytesHex(dir)}`,
    lane_close_epochs: Number(closeEpochs),
    next_lane_id: Number(nextLane),
    listings: Object.fromEntries(mapEntries(listings)),
    lanes: mapEntries(lanes).map(([id, l]) => ({
      lane_id: Number(id), buyer: bytesHex(l[0]), provider: l[1], provider_key: bytesHex(l[2]),
      deposited: Number(l[3]), claimed: Number(l[4]), close_requested_epoch: l[5] === null ? null : Number(l[5]),
      closed: l[6] === true, voucher_key: bytesHex(l[7]),
    })),
  };
}

// ---- writes ----

function walletd() {
  const url = process.env.WALLETD_URL || "http://127.0.0.1:5100/json_rpc";
  const token = process.env.WALLETD_TOKEN ?? (process.env.WALLETD_TOKEN_FILE ? readFileSync(process.env.WALLETD_TOKEN_FILE, "utf8").trim() : undefined);
  let id = 0;
  return async (method, params) => {
    const res = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) },
      body: JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }),
    });
    const body = await res.json();
    if (body.error) throw new Error(`${method}: ${body.error.message}`);
    return body.result;
  };
}

async function accountInfo(rpc, name) {
  const { accounts } = await rpc("accounts.list", { offset: 0, limit: 100 });
  const a = accounts.find((x) => x.account.name === name);
  if (!a) throw new Error(`no walletd account named "${name}"`);
  const index = a.account.owner_key_id?.Derived?.index;
  if (index === undefined) throw new Error(`account "${name}" has no derived key index`);
  return { component: a.account.component_address, index };
}

export const validName = (name) => /^[a-z0-9][a-z0-9_-]{1,31}$/.test(name);
export const cardSubstate = (name) => `nft_${deployment().cards.replace("resource_", "")}_str_${name}`;
const hexKey = (h) => {
  if (!/^[0-9a-f]{64}$/i.test(h)) throw new Error("voucher key must be 32 bytes of hex");
  // The ledger accepts any 32 bytes, but no voucher can be signed for a key that isn't a curve point:
  // the lane could then only be closed, after the close window.
  try { ristretto255.Point.fromBytes(Buffer.from(h, "hex")); } catch { throw new Error("voucher key is not a valid Ristretto public key"); }
  return [...Buffer.from(h, "hex")];
};

/** `name`'s card as the indexer has it: { status, channel } or null. Card state is [agent_key, owner_key,
 *  messaging_address, description, channel, status, nonce, updated_epoch]. */
export async function readCard(name) {
  const got = await indexer(`/substates/${cardSubstate(name)}`);
  const card = got?.state?.up?.substate?.NonFungible?.mutable_data;
  if (!Array.isArray(card)) return null;
  const channel = bytesHex(card[4]);
  return { status: card[5] === 0 ? "active" : "retired", channel: channel ? `component_${channel}` : null };
}

/** A ledger call that reads `provider`'s card. The card is declared read-only, and so is the channel it points at:
 *  reading a card loads the components it references, which walletd doesn't detect. */
async function cardCall(flags, provider, method, args) {
  if (!validName(provider)) throw new Error(`invalid directory name "${provider}"`);
  const card = await readCard(provider);
  if (!card) throw new Error(`"${provider}" is not in the Ootle Agent Directory`);
  if (card.status !== "active") throw new Error(`"${provider}" is retired in the directory`);
  const rpc = walletd();
  const acct = await accountInfo(rpc, flags.account || "agent");
  return submitInstructions(rpc, {
    feeAccount: acct.component, signerIndex: acct.index, instructions: [callMethod(deployment().ledger, method, args)],
    inputs: [cardSubstate(provider), ...(card.channel ? [card.channel] : [])], indexer: deployment().indexer,
    live: flags.live === true, ceiling: Number(flags["max-fee"] ?? 15_000),
  });
}

export const setListing = (flags, name, listing) => cardCall(flags, name, "set_listing", [cbor.text(name), cbor.text(listing)]);
export const openLane = (flags, provider, microtari, voucherKey) =>
  cardCall(flags, provider, "open_lane", [cbor.text(provider), cbor.uint(microtari), cbor.bytes(hexKey(voucherKey))]);

async function main(argv) {
  const [cmd, ...rest] = argv;
  const flags = {}, args = [];
  for (let i = 0; i < rest.length; i++) {
    if (rest[i].startsWith("--")) flags[rest[i].slice(2)] = rest[i + 1] !== undefined && !rest[i + 1].startsWith("--") ? rest[++i] : true;
    else args.push(rest[i]);
  }
  const done = (r, what) => console.log(r.dry_run ? `Dry run accepted (${what}): fee ${r.required_fee} µT. Add --live to send.` : `${what}: ${r.transaction_id} (fee ${r.fee} µT)`);
  switch (cmd) {
    case "config": {
      const l = await readLedger();
      console.log(JSON.stringify({ ledger: deployment().ledger, directory: l.directory, lane_close_epochs: l.lane_close_epochs, lanes: l.next_lane_id - 1, providers_listed: Object.keys(l.listings).length }, null, 2));
      break;
    }
    case "listing": console.log((await readLedger()).listings[args[0]] ?? "(no listing)"); break;
    case "lane": console.log(JSON.stringify((await readLedger()).lanes.find((l) => l.lane_id === Number(args[0])) ?? null, null, 2)); break;
    case "lanes": console.log(JSON.stringify((await readLedger()).lanes.filter((l) => l.provider === args[0]), null, 2)); break;
    case "set-listing":
      if (!flags.name || args.length !== 1) throw new Error('usage: set-listing --account A --name N "<listing>" [--live]');
      done(await setListing(flags, flags.name, args[0]), `${flags.name}'s listing set`);
      break;
    case "open-lane":
      if (args.length !== 3) throw new Error("usage: open-lane --account A <provider> <microtari> <voucher key hex> [--live]");
      done(await openLane(flags, args[0], args[1], args[2]), `lane to ${args[0]} opened`);
      break;
    default:
      console.log(readFileSync(fileURLToPath(import.meta.url), "utf8").split("\n").slice(1, 13).map((l) => l.replace(/^\/\/ ?/, "")).join("\n"));
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2)).catch((e) => { console.error(e.message); process.exit(1); });
}
