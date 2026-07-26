#!/usr/bin/env python3
"""Development-only Fact Console gateway for the community-stack Unix API."""

from __future__ import annotations

import argparse
import hmac
import json
import os
import secrets
import sys
import uuid
from http import HTTPStatus
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "clients" / "python"))
from community_stack_ipc import CoreClient, CoreProtocolError  # noqa: E402

APP_METHODS = {
    "health",
    "fact.assert",
    "fact.query",
    "fact.inspect",
    "fact.history",
    "document.get",
}
ADMIN_METHODS = {"schema.register", "schema.list", "system.doctor", "projection.check"}
MAX_HTTP_BODY = 256 * 1024


class FactConsoleHandler(SimpleHTTPRequestHandler):
    server: "FactConsoleServer"

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, directory=str(Path(__file__).parent), **kwargs)

    def end_headers(self) -> None:
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header(
            "Content-Security-Policy",
            "default-src 'self'; script-src 'self'; style-src 'self'; "
            "connect-src 'self'; img-src 'none'; frame-ancestors 'none'",
        )
        super().end_headers()

    def do_GET(self) -> None:  # noqa: N802
        if not self._valid_host():
            self.send_error(HTTPStatus.BAD_REQUEST, "invalid Host header")
            return
        if self.path == "/":
            self.path = "/index.html"
        if self.path not in {"/index.html", "/app.js", "/styles.css"}:
            self.send_error(HTTPStatus.NOT_FOUND)
            return
        super().do_GET()

    def do_POST(self) -> None:  # noqa: N802
        if (
            not self._valid_host()
            or not self._same_origin()
            or not self._valid_session()
        ):
            self._json(HTTPStatus.FORBIDDEN, {"ok": False, "error": "request rejected"})
            return
        if self.path != "/rpc":
            self._json(HTTPStatus.NOT_FOUND, {"ok": False, "error": "not found"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if length <= 0 or length > MAX_HTTP_BODY:
                raise ValueError("invalid request size")
            request = json.loads(self.rfile.read(length))
            method = request.get("method")
            params = request.get("params", {})
            if not isinstance(params, dict):
                raise ValueError("parameters must be an object")
            if method in APP_METHODS:
                client = self.server.core
            elif method in ADMIN_METHODS and self.server.admin is not None:
                client = self.server.admin
            else:
                raise ValueError("method is not allowed or ADMIN capability is unavailable")
            result = client.call(method, params, f"ui-{uuid.uuid4()}")
            self._json(HTTPStatus.OK, {"ok": True, "result": result})
        except (ValueError, json.JSONDecodeError, CoreProtocolError) as error:
            self._json(HTTPStatus.BAD_REQUEST, {"ok": False, "error": str(error)})

    def log_message(self, format: str, *args: Any) -> None:
        print(f"fact-console: {format % args}", file=sys.stderr)

    def _valid_host(self) -> bool:
        return self.headers.get("Host") in {
            f"127.0.0.1:{self.server.server_port}",
            f"localhost:{self.server.server_port}",
        }

    def _same_origin(self) -> bool:
        origin = self.headers.get("Origin")
        return origin in {
            f"http://127.0.0.1:{self.server.server_port}",
            f"http://localhost:{self.server.server_port}",
        }

    def _valid_session(self) -> bool:
        supplied = self.headers.get("X-Community-Console-Session", "")
        return hmac.compare_digest(supplied, self.server.session_secret)

    def _json(self, status: HTTPStatus, value: dict[str, Any]) -> None:
        body = json.dumps(value, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class FactConsoleServer(ThreadingHTTPServer):
    def __init__(
        self,
        address: tuple[str, int],
        core: CoreClient,
        admin: CoreClient | None,
        session_secret: str,
    ):
        super().__init__(address, FactConsoleHandler)
        self.core = core
        self.admin = admin
        self.session_secret = session_secret


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", required=True, help="community-stack Unix socket")
    parser.add_argument(
        "--token",
        default=os.environ.get("COMMUNITY_APP_TOKEN"),
        help="APP token; prefer COMMUNITY_APP_TOKEN to shell history",
    )
    parser.add_argument(
        "--admin-token",
        default=os.environ.get("COMMUNITY_ADMIN_TOKEN"),
        help="separate ADMIN token; prefer COMMUNITY_ADMIN_TOKEN",
    )
    parser.add_argument("--port", type=int, default=8765)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    if not args.token:
        raise SystemExit("--token or COMMUNITY_APP_TOKEN is required")
    admin = CoreClient(args.socket, args.admin_token) if args.admin_token else None
    session_secret = secrets.token_urlsafe(32)
    server = FactConsoleServer(
        ("127.0.0.1", args.port),
        CoreClient(args.socket, args.token),
        admin,
        session_secret,
    )
    print(
        f"Fact Console: http://127.0.0.1:{server.server_port}/#{session_secret}"
    )
    print("Development-only loopback gateway; press Ctrl-C to stop.")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
