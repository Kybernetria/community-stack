#!/usr/bin/env python3
"""Dependency-free, loopback-only Community Hub development gateway."""
from __future__ import annotations

import argparse
from datetime import date
from dataclasses import asdict
import hmac
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
from pathlib import Path
import secrets
import sys
import uuid

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "clients/python"))
from community_stack_ipc import CoreClient, CoreProtocolError
from community_workspace import Document, Workspace, table_csv

MAX_HTTP_BODY = 256 * 1024
METHODS = frozenset({"health", "document.list", "document.get", "document.changes",
                     "note.save", "table.create", "row.put", "row.delete", "table.csv",
                     "planning.project.put", "planning.task.put", "planning.event.put",
                     "planning.calendar.list", "planning.gantt.get"})


def shape(value, required, optional=()):
    if not isinstance(value, dict) or not set(required) <= value.keys() or value.keys() - set(required) - set(optional):
        raise ValueError("Unexpected or missing fields")


def text(value, maximum=256, empty=False, controls=True):
    if (not isinstance(value, str) or (not empty and not value.strip())
            or len(value.encode("utf-8")) > maximum or '\x00' in value
            or (controls and any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in value))):
        raise ValueError("Invalid text field or length")
    return value


def integer(value):
    if type(value) is not int or not 0 <= value <= 2**53 - 1:
        raise ValueError("Expected a nonnegative safe integer")
    return value


def primitive(value):
    if value is None or type(value) in (str, bool):
        return
    if type(value) is int and -(2**53 - 1) <= value <= 2**53 - 1:
        return
    if type(value) is float and math.isfinite(value):
        return
    raise ValueError("Values must be finite JSON primitives")


def iso_date(value):
    value = text(value, 10)
    if len(value) != 10 or value[4] != "-" or value[7] != "-":
        raise ValueError("Invalid date")
    return date.fromisoformat(value)


class Capture:
    """Build helper mutations without issuing any IPC requests."""
    def call(self, method, params, request_id):
        self.params = params
        return {"revision": params["expected_revision"], "state": {}}


def dispatch(core, request):
    shape(request, {"method", "params"})
    method, p = request["method"], request["params"]
    if not isinstance(method, str) or method not in METHODS:
        raise ValueError("Method is not allowed")
    if method == "health":
        shape(p, set())
        return core.call("health", {}, uuid.uuid4().hex)
    if not isinstance(p, dict):
        raise ValueError("Parameters must be an object")
    community = text(p.get("community_id"))
    workspace = Workspace(core, community)
    if method in {"planning.calendar.list", "planning.gantt.get"}:
        shape(p, {"community_id"})
        return workspace.call(method, limit=100)
    if method in {"planning.project.put", "planning.task.put", "planning.event.put"}:
        kind = method.split(".")[1]
        required = {"community_id", "command_id", "title", "record_id", "status", "description"}
        if kind == "task":
            required |= {"project_id", "progress_percent"}
        if kind == "event":
            required |= {"project_id", "start_date", "end_date_exclusive"}
        shape(p, required)
        description = text(p["description"], 4000, True)
        status = text(p["status"], 32)
        payload = {"idempotency_key": text(p["command_id"], 64),
                   kind + "_id": text(p["record_id"]), "title": text(p["title"], 240),
                   "status": status}
        # The core models description as an optional field and rejects Some("").
        if description.strip():
            payload["description"] = description
        if status not in ("planned", "active", "completed", "cancelled", "archived"):
            raise ValueError("Invalid planning status")
        if kind != "project":
            project_id = p["project_id"]
            if project_id is None or (isinstance(project_id, str) and not project_id.strip()):
                payload["project_id"] = None
            else:
                payload["project_id"] = text(project_id)
        if kind == "task":
            progress = integer(p["progress_percent"])
            if progress > 100:
                raise ValueError("Progress must be 0–100")
            payload["progress_percent"] = progress
        if kind == "event":
            start = iso_date(p["start_date"])
            end = iso_date(p["end_date_exclusive"])
            if end <= start:
                raise ValueError("Event end must be after start")
            payload["timing"] = {"kind": "all_day", "start_date": start.isoformat(), "end_date_exclusive": end.isoformat()}
        return workspace.call(method, **payload)
    if method == "document.list":
        shape(p, {"community_id"}, {"after"})
        after = p.get("after")
        if after is not None:
            text(after)
        return workspace.call(method, after=after, limit=100)
    if method == "document.changes":
        shape(p, {"community_id"}, {"after"})
        return workspace.changes(after=integer(p.get("after", 0)), limit=50)
    if method in {"document.get", "table.csv"}:
        shape(p, {"community_id", "document_id"})
        doc = workspace.get(text(p["document_id"]))
        return {"csv": table_csv(doc)} if method == "table.csv" else asdict(doc)
    base = {"community_id", "document_id", "command_id"}
    extra = {"note.save": {"snapshot", "title", "body", "metadata"},
             "table.create": {"title", "columns"},
             "row.put": {"snapshot", "row_id", "values"},
             "row.delete": {"snapshot", "row_id"}}[method]
    shape(p, base | extra)
    doc_id = text(p["document_id"])
    command = text(p["command_id"], 64)
    capture = Capture()
    builder = Workspace(capture, community)
    if method == "table.create":
        columns = p["columns"]
        if not isinstance(columns, dict) or not 1 <= len(columns) <= 32:
            raise ValueError("Use 1–32 columns")
        for key, label in columns.items():
            text(key, 64)
            text(label, 120)
        builder.create_table(doc_id, text(p["title"], 240), columns, command_id=command)
    else:
        snapshot = p["snapshot"]
        shape(snapshot, {"revision", "state"})
        revision = integer(snapshot["revision"])
        state = snapshot["state"]
        if not isinstance(state, dict) or not isinstance(state.get("meta", {}), dict):
            raise ValueError("Invalid document snapshot")
        doc = Document(community, doc_id, revision, state)
        if method == "note.save":
            if not isinstance(state.get("body", ""), str):
                raise ValueError("Body must be text")
            metadata = p["metadata"]
            if not isinstance(metadata, dict) or len(metadata) > 32:
                raise ValueError("Metadata must be an object with at most 32 fields")
            for key, value in metadata.items():
                text(key, 64)
                if key in {"kind", "title"}:
                    raise ValueError("Title and kind have dedicated fields")
                primitive(value)
            builder.save_note(doc, text(p["title"], 240), text(p["body"], 100000, True, controls=False), command_id=command)
            mutations = capture.params["mutations"]
            for key in sorted(state.get("meta", {})):
                if key not in {"kind", "title"} and key not in metadata:
                    mutations.append({"op": "map_delete", "container": "meta", "key": key})
            mutations.extend({"op": "map_set", "container": "meta", "key": key, "value": value}
                             for key, value in sorted(metadata.items()))
        else:
            for field in ("columns", "rows", "cells"):
                if not isinstance(state.get(field, {}), dict):
                    raise ValueError("Invalid table snapshot")
            text(p["row_id"], 64)
            if method == "row.put":
                if not isinstance(p["values"], dict):
                    raise ValueError("Row values must be an object")
                for value in p["values"].values():
                    primitive(value)
                builder.put_row(doc, p["row_id"], p["values"], command_id=command)
            else:
                builder.delete_row(doc, p["row_id"], command_id=command)
    # The exact submitted snapshot builds the exact same mutation on every retry.
    result = core.call("document.mutate", capture.params, uuid.uuid4().hex)
    return {"community_id": community, "document_id": doc_id,
            "revision": result["revision"], "state": result["state"]}


class Handler(BaseHTTPRequestHandler):
    def setup(self):
        super().setup()
        self.connection.settimeout(20)

    def end_headers(self):
        for key, value in {
            "Cache-Control": "no-store", "X-Content-Type-Options": "nosniff",
            "Referrer-Policy": "no-referrer", "X-Frame-Options": "DENY",
            "Permissions-Policy": "camera=(), microphone=(), geolocation=()",
            "Content-Security-Policy": "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        }.items():
            self.send_header(key, value)
        super().end_headers()

    def valid_host(self):
        return self.headers.get_all("Host") == [f"127.0.0.1:{self.server.server_port}"]

    def reply(self, status, value):
        body = json.dumps(value, allow_nan=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def error(self, status, code, message, **extra):
        self.reply(status, {"ok": False, "error": {"code": code, "message": message, **extra}})

    def do_GET(self):
        if not self.valid_host():
            self.error(403, "FORBIDDEN", "Request rejected")
            return
        files = {"/": ("index.html", "text/html"), "/index.html": ("index.html", "text/html"),
                 "/app.js": ("app.js", "text/javascript"), "/styles.css": ("styles.css", "text/css")}
        if self.path not in files:
            self.error(404, "NOT_FOUND", "Not found")
            return
        name, mime = files[self.path]
        body = Path(__file__).with_name(name).read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", mime + "; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        secret = self.headers.get("X-Community-Hub-Session", "")
        if (not self.valid_host()
                or self.headers.get_all("Origin") != [f"http://127.0.0.1:{self.server.server_port}"]
                or not hmac.compare_digest(secret.encode(), self.server.session_secret.encode())):
            self.error(403, "FORBIDDEN", "Open the printed session URL to connect")
            return
        if self.path != "/api":
            self.error(404, "NOT_FOUND", "Not found")
            return
        try:
            if self.headers.get("Transfer-Encoding") or len(self.headers.get_all("Content-Length", [])) != 1:
                raise ValueError("Invalid request framing")
            length = int(self.headers["Content-Length"])
            if not 0 < length <= MAX_HTTP_BODY:
                raise ValueError("Request exceeds 256 KiB or is empty")
            if self.headers.get_content_type() != "application/json":
                raise ValueError("Expected application/json")
            body = self.rfile.read(length)
            if len(body) != length:
                raise ValueError("Incomplete request")
            def reject_constant(value):
                raise ValueError("Nonfinite JSON number")
            request = json.loads(body, parse_constant=reject_constant)
            result = dispatch(self.server.core, request)
            self.reply(200, {"ok": True, "result": result})
        except CoreProtocolError as error:
            code = error.code
            status = 409 if code in {"REVISION_CONFLICT", "IDEMPOTENCY_CONFLICT"} else 502
            message = "Core API rejected the request. Check the APP capability and core connection."
            extra = {}
            if code == "REVISION_CONFLICT":
                message = "A newer revision exists. Your draft was not saved. Review the latest document before editing again."
                try:
                    p = request["params"]
                    extra["latest"] = asdict(Workspace(self.server.core, p["community_id"]).get(p["document_id"]))
                except CoreProtocolError:
                    pass
            if code == "PROTOCOL_ERROR":
                message = "Core is offline or the local API connection failed. Start the core, then retry."
            self.error(status, code, message, **extra)
        except (ValueError, TypeError, KeyError, UnicodeError, RecursionError):
            self.error(400, "INVALID_REQUEST", "Invalid request shape, value, or size")
        except (OSError, TimeoutError):
            self.error(408, "TIMEOUT", "Request timed out; retry the exact pending write")

    def log_message(self, *args):
        pass  # No URLs, capabilities, payloads, or document contents in access logs.


class HubServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, port, core, session_secret):
        self.core, self.session_secret = core, session_secret
        super().__init__(("127.0.0.1", port), Handler)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", required=True)
    parser.add_argument("--token", default=os.environ.get("COMMUNITY_APP_TOKEN"))
    parser.add_argument("--port", type=int, default=8766)
    args = parser.parse_args()
    if not args.token:
        parser.error("COMMUNITY_APP_TOKEN or --token is required")
    server = HubServer(args.port, CoreClient(args.socket, args.token), secrets.token_urlsafe(32))
    print(f"Community Hub: http://127.0.0.1:{server.server_port}/#{server.session_secret}", flush=True)
    print("Development gateway · press Ctrl-C to stop.", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
