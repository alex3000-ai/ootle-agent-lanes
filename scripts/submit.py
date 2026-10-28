#!/usr/bin/env python3
"""Submit a transaction manifest through a local Ootle wallet daemon.

    scripts/submit.py MANIFEST [--var NAME=VALUE ...] [--signer-index N] [--max-fee MICROTARI] [--live]

Without --live it only dry-runs. With --live it dry-runs first, submits only if the dry run is
accepted, waits for the result, and prints the components, resources and lanes.* events it touched.

--signer-index is the walletd account key index (`owner_key_id.Derived.index` in accounts.list)
that seals the transaction; the account paying in `fee_main` must belong to that key.

Environment: WALLETD_URL (default http://127.0.0.1:5100/json_rpc), WALLETD_TOKEN (optional bearer
token; otherwise a short-lived Admin token is requested, which needs `--authentication none`).
"""

import argparse
import json
import os
import re
import sys
import urllib.request

URL = os.environ.get("WALLETD_URL", "http://127.0.0.1:5100/json_rpc")


def rpc(method, params, token=None, timeout=300):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
    request = urllib.request.Request(URL, body, {"content-type": "application/json"})
    if token:
        request.add_header("authorization", f"Bearer {token}")
    with urllib.request.urlopen(request, timeout=timeout) as response:
        reply = json.load(response)
    if "error" in reply:
        sys.exit(f"{method}: {reply['error'].get('message', reply['error'])}")
    return reply["result"]


def token():
    if os.environ.get("WALLETD_TOKEN"):
        return os.environ["WALLETD_TOKEN"]
    return rpc("auth.request", {"permissions": ["Admin"], "credentials": "None"})["token"]


def summarize(result):
    text = json.dumps(result)
    components = sorted(set(re.findall(r"component_[0-9a-f]{64}", text)))
    resources = sorted(set(re.findall(r"resource_[0-9a-f]{64}", text)) - {"resource_" + "01" * 32})
    events = []

    def walk(node):
        if isinstance(node, dict):
            if str(node.get("topic", "")).startswith("lanes.") and node not in events:
                events.append(node)
            for value in node.values():
                walk(value)
        elif isinstance(node, list):
            for value in node:
                walk(value)

    walk(result)
    return components, resources, [{"topic": e["topic"], **e.get("payload", {})} for e in events]


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("manifest")
    parser.add_argument("--var", action="append", default=[], metavar="NAME=VALUE")
    parser.add_argument("--signer-index", type=int, required=True)
    parser.add_argument("--max-fee", type=int, default=20_000)
    parser.add_argument("--live", action="store_true")
    args = parser.parse_args()

    variables = dict(v.split("=", 1) for v in args.var)
    params = {
        "manifest": open(args.manifest).read(),
        "variables": variables,
        "seal_signer_key_id": {"Derived": {"index": args.signer_index, "key_branch": "account"}},
        "max_fee": args.max_fee,
        "dry_run": True,
    }
    bearer = token()
    dry = rpc("transactions.submit_manifest", params, bearer)
    print(f"dry run accepted, required fee {dry.get('required_fees')} microtari")
    if not args.live:
        return

    params["dry_run"] = False
    submitted = rpc("transactions.submit_manifest", params, bearer)
    tx = submitted["transaction_id"]
    result = rpc("transactions.wait_result", {"transaction_id": tx, "timeout_secs": 240}, bearer)
    print(f"tx {tx}: {result.get('status')}, fee {result.get('final_fee')} microtari")
    if result.get("status") != "Accepted":
        sys.exit(json.dumps(result)[:2000])
    components, resources, events = summarize(result)
    print(json.dumps({"components": components, "resources": resources, "events": events}, indent=2))


if __name__ == "__main__":
    main()
