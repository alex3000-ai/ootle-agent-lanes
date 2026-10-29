// Offline checks for the lanes client: voucher signing against the template's format, and the client's own helpers.
//   npm test

import assert from "node:assert/strict";
import { test } from "node:test";
import { keygen, publicKeyOf, signVoucher, verifyVoucher, voucherMessage } from "./voucher.mjs";
import { laneSubstate, validName } from "./lanes.mjs";
import { existsSync, readFileSync } from "node:fs";

const DIRECTORY_TX = "../../ootle-agent-directory/client/tx.mjs";

const LEDGER = "component_ade394f14d3429a65c1d15537d85b2babaf993cb8da2975a6cc481652f2a191b";

// Claimed on Esmeralda by the v0.3 template (lane 1, transaction a9626f01…a398): the template verified this signature.
const ACCEPTED = {
  ledger: LEDGER, lane_id: "1", spent_cum: "5000",
  public_key: "38d097e323126646ee095c251f617bc5392768c9bcfe49ab2162365566684454",
  public_nonce: "1a607b7ecfc9219dad2c17a9e36375fcf3efa60a989759e3e362e29251e8031d",
  signature: "bc0f0e22e1509d44bd545984c972301752c083e55f13ba500f208e0ad942ae0b",
};

test("a voucher the template accepted on-chain verifies", () => {
  assert.equal(verifyVoucher(ACCEPTED), true);
});

test("changing any signed field breaks the voucher", () => {
  for (const [field, value] of [["lane_id", "2"], ["spent_cum", "5001"], ["ledger", "component_" + "ab".repeat(32)], ["public_key", "dceec2e9d51b27c2f41b16c0b65d70ba402b632f1112c2ec9ae0da0e702a4379"]]) {
    assert.equal(verifyVoucher({ ...ACCEPTED, [field]: value }), false, field);
  }
  assert.equal(verifyVoucher({ ...ACCEPTED, signature: "00".repeat(32) }), false);
  assert.equal(verifyVoucher({ ...ACCEPTED, public_nonce: "zz" }), false, "garbage is invalid, not an exception");
});

test("sign and verify round-trip with a fresh key", () => {
  const { secretKey, publicKey } = keygen();
  assert.equal(publicKeyOf(secretKey), publicKey);
  const v = signVoucher(secretKey, LEDGER, 7, 123456789012n);
  assert.equal(v.public_key, publicKey);
  assert.equal(verifyVoucher(v), true);
});

test("the message is <ledger>|<lane id>|<spent_cum>, as lane_voucher_message builds it", () => {
  assert.equal(new TextDecoder().decode(voucherMessage(LEDGER, 3, "0400")), `${LEDGER}|3|400`);
  assert.throws(() => voucherMessage("component_xyz", 1, 1), /ledger must be/);
});

test("lane substates and directory names", () => {
  assert.equal(laneSubstate({ lanes: "resource_" + "cd".repeat(32) }, 22), `nft_${"cd".repeat(32)}_u64_22`);
  for (const ok of ["opencodeagent", "hex-wright", "a1"]) assert.equal(validName(ok), true, ok);
  for (const bad of ["", "a", "Upper", "-lead", "x".repeat(33), "sp ace"]) assert.equal(validName(bad), false, bad);
});

test("tx.mjs is the directory repo's copy (when it's checked out next to this one)", { skip: !existsSync(DIRECTORY_TX) }, () => {
  assert.equal(readFileSync("tx.mjs", "utf8").split("\n").slice(1).join("\n"), readFileSync(DIRECTORY_TX, "utf8"));
});
