#!/usr/bin/env node
// Lane vouchers for Ootle Agent Lanes, signed on your own machine.
//
//   node voucher.mjs keygen --out key.hex            new voucher key (secret to the file, public key to stdout)
//   node voucher.mjs sign --key-file key.hex <ledger component> <lane id> <spent_cum microtari>
//
// A voucher authorises the provider to claim `spent_cum` (cumulative, in microtari) from the lane.
// The signature matches LaneLedger::claim: Ristretto Schnorr with
//   e = Blake2b-512(domain || R || P || message) mod l,   s = r + e·k
// over the message "<ledger>|<lane id>|<spent_cum>", domain "ootle-agent-lane-voucher-v1".

import { readFileSync, writeFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { pathToFileURL } from "node:url";
import { ristretto255 } from "@noble/curves/ed25519.js";
import { blake2b } from "@noble/hashes/blake2.js";

const Point = ristretto255.Point;
const ORDER = Point.Fn.ORDER;
const enc = new TextEncoder();

export const LANE_VOUCHER_DOMAIN = "ootle-agent-lane-voucher-v1";

const hex = (b) => Buffer.from(b).toString("hex");
const fromHex = (h) => {
  const clean = h.trim().replace(/^0x/i, "");
  if (!/^([0-9a-fA-F]{2})*$/.test(clean)) throw new Error("invalid hex");
  return Uint8Array.from(Buffer.from(clean, "hex"));
};
const leToBigInt = (b) => b.reduceRight((n, byte) => (n << 8n) | BigInt(byte), 0n);
const bigIntToLe32 = (n) => {
  const out = new Uint8Array(32);
  for (let i = 0; i < 32; i++, n >>= 8n) out[i] = Number(n & 0xffn);
  return out;
};
const randomScalar = () => {
  for (;;) {
    const k = leToBigInt(randomBytes(64)) % ORDER;
    if (k !== 0n) return k;
  }
};

/** The exact bytes the buyer signs, as `lanes_ledger::lane_voucher_message`. */
export function voucherMessage(ledger, laneId, spentCum) {
  if (!/^component_[0-9a-f]{64}$/.test(ledger)) throw new Error("ledger must be a component_<64 hex> address");
  return enc.encode(`${ledger}|${BigInt(laneId)}|${BigInt(spentCum)}`);
}

function challenge(nonce, publicKey, message) {
  const h = blake2b.create({ dkLen: 64 });
  h.update(enc.encode(LANE_VOUCHER_DOMAIN)).update(nonce).update(publicKey).update(message);
  return leToBigInt(h.digest()) % ORDER;
}

export function keygen() {
  const secret = randomScalar();
  return { secretKey: hex(bigIntToLe32(secret)), publicKey: hex(Point.BASE.multiply(secret).toBytes()) };
}

export function publicKeyOf(secretKeyHex) {
  return hex(Point.BASE.multiply(parseSecret(secretKeyHex)).toBytes());
}

function parseSecret(secretKeyHex) {
  const bytes = fromHex(secretKeyHex);
  if (bytes.length !== 32) throw new Error("secret key must be 32 bytes");
  const k = leToBigInt(bytes);
  if (k === 0n || k >= ORDER) throw new Error("secret key is not a canonical non-zero scalar");
  return k;
}

/** Signs a voucher. Returns its fields as strings, ready for `lanes.mjs claim`. */
export function signVoucher(secretKeyHex, ledger, laneId, spentCum) {
  const k = parseSecret(secretKeyHex);
  const publicKey = Point.BASE.multiply(k).toBytes();
  const r = randomScalar();
  const nonce = Point.BASE.multiply(r).toBytes();
  const e = challenge(nonce, publicKey, voucherMessage(ledger, laneId, spentCum));
  const s = bigIntToLe32((r + e * k) % ORDER);
  return {
    ledger,
    lane_id: String(laneId),
    spent_cum: String(spentCum),
    public_key: hex(publicKey),
    public_nonce: hex(nonce),
    signature: hex(s),
  };
}

/** Checks a voucher the way the template does, so a provider can verify before doing the work. */
export function verifyVoucher({ ledger, lane_id, spent_cum, public_key, public_nonce, signature }) {
  try {
    const P = Point.fromBytes(fromHex(public_key));
    const R = Point.fromBytes(fromHex(public_nonce));
    const s = leToBigInt(fromHex(signature));
    if (s >= ORDER) return false;
    const e = challenge(fromHex(public_nonce), fromHex(public_key), voucherMessage(ledger, lane_id, spent_cum));
    const lhs = s === 0n ? Point.ZERO : Point.BASE.multiply(s);
    return lhs.equals(e === 0n ? R : R.add(P.multiply(e)));
  } catch {
    return false;
  }
}

function main(argv) {
  const [command, ...rest] = argv;
  const flag = (name) => {
    const i = rest.indexOf(name);
    if (i === -1) return undefined;
    const [, value] = rest.splice(i, 2);
    return value;
  };
  if (command === "keygen") {
    const out = flag("--out");
    if (!out) usage(); // the secret key is never printed
    const { secretKey, publicKey } = keygen();
    writeFileSync(out, secretKey + "\n", { mode: 0o600, flag: "wx" });
    console.log(JSON.stringify({ public_key: publicKey }, null, 2));
    return;
  }
  if (command === "sign") {
    const keyFile = flag("--key-file");
    const secret = keyFile ? readFileSync(keyFile, "utf8") : process.env.LANE_VOUCHER_KEY;
    const [ledger, laneId, spentCum] = rest;
    if (!secret || !ledger || laneId === undefined || spentCum === undefined) usage();
    const voucher = signVoucher(secret, ledger, laneId, spentCum);
    if (!verifyVoucher(voucher)) throw new Error("self-check failed");
    console.log(JSON.stringify(voucher, null, 2));
    return;
  }
  if (command === "verify") {
    const voucher = JSON.parse(rest[0] ?? readFileSync(0, "utf8"));
    const ok = verifyVoucher(voucher);
    console.log(ok ? "valid" : "INVALID");
    process.exit(ok ? 0 : 1);
  }
  usage();
}

function usage() {
  console.error(`Usage:
  voucher.mjs keygen --out key.hex
  voucher.mjs sign --key-file key.hex <ledger component> <lane id> <spent_cum microtari>   (or LANE_VOUCHER_KEY=<hex>)
  voucher.mjs verify '<voucher json>'                                                     (or the JSON on stdin)`);
  process.exit(2);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main(process.argv.slice(2));
