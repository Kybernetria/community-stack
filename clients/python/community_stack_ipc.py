"""Bounded client for community-stack's versioned local IPC protocol."""

from __future__ import annotations

import json
import socket
import struct
from dataclasses import dataclass
from typing import Any

MAX_RESPONSE = 4 * 1024 * 1024


class CoreProtocolError(RuntimeError):
    pass


@dataclass(frozen=True)
class CoreClient:
    socket_path: str
    token: str
    timeout: float = 15.0

    def call(self, method: str, params: dict[str, Any], request_id: str) -> Any:
        request = json.dumps(
            {"v": 1, "id": request_id, "token": self.token, "method": method, "params": params},
            separators=(",", ":"),
        ).encode("utf-8")
        if len(request) > 1024 * 1024:
            raise CoreProtocolError("request exceeds local IPC limit")

        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.settimeout(self.timeout)
                stream.connect(self.socket_path)
                stream.sendall(struct.pack(">I", len(request)) + request)
                length = struct.unpack(">I", self._read_exact(stream, 4))[0]
                if length == 0 or length > MAX_RESPONSE:
                    raise CoreProtocolError(f"invalid response frame length {length}")
                response = json.loads(self._read_exact(stream, length))
        except CoreProtocolError:
            raise
        except (OSError, ValueError, struct.error) as error:
            raise CoreProtocolError(f"local core is unavailable: {error}") from error

        if response.get("id") != request_id or response.get("v") != 1:
            raise CoreProtocolError("response correlation/version mismatch")
        if not response.get("ok"):
            error = response.get("error", {})
            raise CoreProtocolError(f"{error.get('code', 'UNKNOWN')}: {error.get('message', 'request failed')}")
        return response["result"]

    @staticmethod
    def _read_exact(stream: socket.socket, length: int) -> bytes:
        output = bytearray()
        while len(output) < length:
            chunk = stream.recv(length - len(output))
            if not chunk:
                raise CoreProtocolError("unexpected end of local IPC stream")
            output.extend(chunk)
        return bytes(output)
