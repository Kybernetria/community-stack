#!/usr/bin/env python3
"""Bounded fixture loader using only the native toolkit local API.

The bundled data is a research snapshot, not timeless product certification.
This utility never opens SQLite and never writes projections directly.
"""

import argparse
import json
import os
import socket
import struct
from pathlib import Path

ROOT = Path(__file__).parents[1]
FIXTURES = ROOT / "tests" / "fixtures" / "toolkit"


def call(sock, token, number, method, params):
    request = json.dumps({"v": 1, "id": f"toolkit-seed-{number}", "token": token,
                          "method": method, "params": params}, separators=(",", ":")).encode()
    sock.sendall(struct.pack(">I", len(request)) + request)
    size = struct.unpack(">I", receive(sock, 4))[0]
    response = json.loads(receive(sock, size))
    if not response["ok"]:
        raise RuntimeError(f"{method}: {response['error']['code']}: {response['error']['message']}")
    return response["result"]


def receive(sock, size):
    data = b""
    while len(data) < size:
        chunk = sock.recv(size - len(data))
        if not chunk:
            raise RuntimeError("local API closed the connection")
        data += chunk
    return data


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--token", default=os.environ.get("COMMUNITY_APP_TOKEN"))
    parser.add_argument("--community", required=True)
    parser.add_argument("--max-records", type=int, default=500)
    args = parser.parse_args()
    if not args.token:
        parser.error("COMMUNITY_APP_TOKEN or --token is required")
    if not 1 <= args.max_records <= 1000:
        parser.error("--max-records must be between 1 and 1000")

    taxonomy = json.loads((FIXTURES / "taxonomy-0.1.json").read_text())
    corpus = json.loads((FIXTURES / "corpus-0.1.json").read_text())
    total = len(taxonomy["concepts"]) + len(corpus["tools"]) + len(corpus["assertions"])
    if total > args.max_records:
        raise SystemExit(f"fixture has {total} records, above --max-records")

    number = 0
    with socket.socket(socket.AF_UNIX) as sock:
        sock.connect(args.socket)
        for concept in taxonomy["concepts"]:
            number += 1
            call(sock, args.token, number, "toolkit.schema.add", {
                "community_id": args.community,
                "idempotency_key": f"fixture-concept-{number:04}", **concept})
        for tool in corpus["tools"]:
            number += 1
            call(sock, args.token, number, "toolkit.tool.add", {
                "community_id": args.community,
                "idempotency_key": f"fixture-tool-{number:04}", **tool})
        for index, assertion in enumerate(corpus["assertions"], 1):
            number += 1
            call(sock, args.token, number, "toolkit.assert", {
                "community_id": args.community,
                "idempotency_key": f"fixture-assertion-{index:04}",
                "assertion_id": f"fixture-{index:04}", **assertion})
    print(json.dumps({"ok": True, "records": total, "fixture_notice": corpus["fixture_notice"]}))


if __name__ == "__main__":
    main()
