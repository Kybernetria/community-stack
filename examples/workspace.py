#!/usr/bin/env python3
"""Notes and sparse tables through the local protocol."""
import argparse
import json
import os
from pathlib import Path
import sys
import uuid
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "clients" / "python"))
from community_stack_ipc import CoreClient, CoreProtocolError
from community_workspace import Workspace, table_csv

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", default="./data/community.sock")
    parser.add_argument("--community", required=True)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("list")
    for name in ("get", "csv"):
        sub.add_parser(name).add_argument("document")
    note = sub.add_parser("note")
    note.add_argument("document")
    note.add_argument("--title", required=True)
    note.add_argument("--file", type=Path, required=True)
    table = sub.add_parser("table")
    table.add_argument("document")
    table.add_argument("--title", required=True)
    table.add_argument("--columns", required=True, help='JSON: {"item":"Item","quantity":"Quantity"}')
    row = sub.add_parser("row")
    row.add_argument("document")
    row.add_argument("row_id")
    row.add_argument("--values", required=True, help="JSON keyed by column ID")
    sub.add_parser("changes").add_argument("--after", type=int, default=0)
    args = parser.parse_args()
    token = os.environ.get("COMMUNITY_APP_TOKEN")
    if not token:
        parser.error("set COMMUNITY_APP_TOKEN to an APP capability")
    workspace = Workspace(CoreClient(args.socket, token), args.community)
    command_id = uuid.uuid4().hex
    if args.command == "list": result = list(workspace.documents())
    elif args.command == "get": result = workspace.get(args.document).__dict__
    elif args.command == "note":
        result = workspace.save_note(workspace.get(args.document), args.title, args.file.read_text(encoding="utf-8"), command_id=command_id).__dict__
    elif args.command == "table":
        result = workspace.create_table(args.document, args.title, json.loads(args.columns), command_id=command_id).__dict__
    elif args.command == "row":
        result = workspace.put_row(workspace.get(args.document), args.row_id, json.loads(args.values), command_id=command_id).__dict__
    elif args.command == "csv":
        print(table_csv(workspace.get(args.document)), end="")
        return
    else: result = workspace.changes(args.after)
    print(json.dumps(result, ensure_ascii=False, indent=2))

if __name__ == "__main__":
    try: main()
    except (CoreProtocolError, ValueError, OSError) as error: sys.exit(str(error))
