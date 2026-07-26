#!/usr/bin/env python3
"""Reticulum sidecar baseline: identity, announces, Links, Channel negotiation.

Durable operation transfer is intentionally gated until the manifest/bundle interop
fixtures are implemented. This process never decodes, authorizes, or applies records.
"""

from __future__ import annotations

import argparse
import os
import signal
import stat
import sys
import threading
import time
from pathlib import Path
from typing import Any

import RNS
from RNS.vendor import umsgpack

CLIENTS_DIR = Path(__file__).resolve().parents[1] / "clients" / "python"
sys.path.insert(0, str(CLIENTS_DIR))
from community_stack_ipc import CoreClient  # noqa: E402

APP_NAME = "community_stack"
ASPECT = "sync_v1"
MAX_ACTIVE_LINKS = 32
MAX_LINK_IDLE_SECONDS = 300
MAX_HELLO_BYTES = 4096


class Hello(RNS.MessageBase):
    MSGTYPE = 0x4301

    def __init__(self, data: dict[str, Any] | None = None):
        self.data = data or {}

    def pack(self) -> bytes:
        return umsgpack.packb(self.data)

    def unpack(self, raw: bytes) -> None:
        try:
            self.data = umsgpack.unpackb(raw) if len(raw) <= MAX_HELLO_BYTES else None
        except Exception:  # Reticulum callback boundary: malformed peer input.
            self.data = None


class HelloAck(RNS.MessageBase):
    MSGTYPE = 0x4302

    def __init__(self, data: dict[str, Any] | None = None):
        self.data = data or {}

    def pack(self) -> bytes:
        return umsgpack.packb(self.data)

    def unpack(self, raw: bytes) -> None:
        try:
            self.data = umsgpack.unpackb(raw) if len(raw) <= MAX_HELLO_BYTES else None
        except Exception:  # Reticulum callback boundary: malformed peer input.
            self.data = None


class Bridge:
    def __init__(self, args: argparse.Namespace):
        self.args = args
        self.stop = threading.Event()
        self.links: dict[Any, float] = {}
        self.core = CoreClient(args.core_socket, args.token)
        RNS.Reticulum(args.rns_config)
        self.identity = self._identity(Path(args.identity))
        self.destination = RNS.Destination(
            self.identity,
            RNS.Destination.IN,
            RNS.Destination.SINGLE,
            APP_NAME,
            ASPECT,
        )
        self.destination.set_link_established_callback(self._connected)

    @staticmethod
    def _identity(path: Path):
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        if path.parent.stat().st_mode & 0o077:
            raise RuntimeError(f"Reticulum identity directory must be mode 0700: {path.parent}")
        try:
            metadata = path.lstat()
        except FileNotFoundError:
            metadata = None
        if metadata is not None:
            if (
                path.is_symlink()
                or not stat.S_ISREG(metadata.st_mode)
                or metadata.st_mode & 0o077
            ):
                raise RuntimeError(
                    f"Reticulum identity must be a regular non-symlink mode-0600 file: {path}"
                )
            identity = RNS.Identity.from_file(str(path))
            if identity is None:
                raise RuntimeError(f"could not load Reticulum identity {path}")
            return identity
        identity = RNS.Identity()
        identity.to_file(str(path))
        os.chmod(path, 0o600)
        return identity

    def _connected(self, link) -> None:
        if len(self.links) >= MAX_ACTIVE_LINKS:
            RNS.log("rejecting Link: active Link limit reached", RNS.LOG_WARNING)
            link.teardown()
            return
        self.links[link] = time.monotonic()
        link.set_link_closed_callback(self._closed)
        channel = link.get_channel()
        channel.register_message_type(Hello)
        channel.register_message_type(HelloAck)
        channel.add_message_handler(
            lambda message: self._message(link, channel, message)
        )
        RNS.log(f"community-stack Link established; channel MDU={channel.mdu}")

    def _closed(self, link) -> None:
        self.links.pop(link, None)

    def _message(self, link, channel, message) -> bool:
        if not isinstance(message, Hello):
            return False
        self.links[link] = time.monotonic()
        offered = message.data
        if not isinstance(offered, dict) or len(offered) > 8:
            RNS.log("peer sent malformed Hello", RNS.LOG_WARNING)
            return True
        versions = offered.get("protocol_versions", [])
        if (
            not isinstance(versions, list)
            or len(versions) > 8
            or any(not isinstance(version, int) for version in versions)
        ):
            RNS.log("peer sent malformed protocol versions", RNS.LOG_WARNING)
            return True
        if 1 not in versions:
            RNS.log("peer offered no supported sync protocol", RNS.LOG_WARNING)
            return True
        ack = HelloAck(
            {
                "protocol_version": 1,
                "schema_versions": [1],
                "codecs": ["raw", "zstd"],
                "max_control_frame": min(channel.mdu, self.args.max_control_frame),
                "max_bundle": self.args.max_bundle,
                "profile": self.args.profile,
                "operation_transfer": False,
            }
        )
        if len(ack.pack()) > channel.mdu:
            RNS.log("negotiated HelloAck exceeds Channel MDU", RNS.LOG_ERROR)
            return True
        channel.send(ack)
        return True

    def run(self) -> None:
        health = self.core.call("health", {}, "bridge-startup")
        RNS.log(f"community core status={health['status']}")
        RNS.log(f"sync destination {RNS.prettyhexrep(self.destination.hash)}")
        self.destination.announce(
            app_data=umsgpack.packb({"v": 1, "profile": self.args.profile})
        )
        next_announce = time.monotonic() + self.args.announce_interval
        while not self.stop.wait(1.0):
            now = time.monotonic()
            for link, last_activity in list(self.links.items()):
                if now - last_activity > MAX_LINK_IDLE_SECONDS:
                    self.links.pop(link, None)
                    try:
                        link.teardown()
                    except Exception as error:
                        RNS.log(f"failed to close idle Link: {error}", RNS.LOG_WARNING)
            if now >= next_announce:
                self.destination.announce(
                    app_data=umsgpack.packb({"v": 1, "profile": self.args.profile})
                )
                next_announce = time.monotonic() + self.args.announce_interval


def _secret_file(path: Path) -> str:
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    try:
        metadata = os.fstat(descriptor)
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_mode & 0o077:
            raise ValueError("token file must be a regular mode-0600 file")
        if metadata.st_size > 129:
            raise ValueError("token file is too large")
        token = os.read(descriptor, 129).decode("ascii").strip()
    finally:
        os.close(descriptor)
    if len(token) != 64 or any(character not in "0123456789abcdefABCDEF" for character in token):
        raise ValueError("token file must contain one 32-byte hexadecimal token")
    return token


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core-socket", required=True)
    parser.add_argument(
        "--token-file",
        default=os.environ.get("COMMUNITY_TRANSPORT_TOKEN_FILE"),
        help="mode-0600 file containing the TRANSPORT token",
    )
    parser.add_argument("--identity", required=True)
    parser.add_argument("--rns-config", default=None, help="Reticulum config directory; shared rnsd is preferred")
    parser.add_argument("--profile", choices=("radio", "balanced", "fast"), default="radio")
    parser.add_argument("--max-control-frame", type=int, default=384)
    parser.add_argument("--max-bundle", type=int, default=65536)
    parser.add_argument("--announce-interval", type=int, default=1800)
    args = parser.parse_args()
    if not args.token_file:
        parser.error("--token-file or COMMUNITY_TRANSPORT_TOKEN_FILE is required")
    if not 128 <= args.max_control_frame <= 4096:
        parser.error("--max-control-frame must be between 128 and 4096")
    if not 1024 <= args.max_bundle <= 4 * 1024 * 1024:
        parser.error("--max-bundle must be between 1024 and 4194304")
    if not 60 <= args.announce_interval <= 86400:
        parser.error("--announce-interval must be between 60 and 86400")
    args.token = _secret_file(Path(args.token_file))
    return args


def main() -> None:
    bridge = Bridge(parse_args())
    signal.signal(signal.SIGINT, lambda *_: bridge.stop.set())
    signal.signal(signal.SIGTERM, lambda *_: bridge.stop.set())
    bridge.run()


if __name__ == "__main__":
    main()
