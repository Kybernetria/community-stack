#!/usr/bin/env python3
"""Real binary/Unix IPC smoke test, including stale edits and device recovery."""
import argparse
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "clients" / "python"))
from community_stack_ipc import CoreClient, CoreProtocolError
from community_workspace import Workspace, table_csv

def run(binary, *args):
    return subprocess.run([str(binary), *map(str, args)], check=True, capture_output=True, text=True).stdout

def start(binary, directory):
    process = subprocess.Popen([str(binary), "serve", "--data-dir", str(directory)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    client = CoreClient(str(directory / "community.sock"), "")
    for _ in range(100):
        if process.poll() is not None:
            raise RuntimeError(process.stderr.read())
        try:
            client.call("health", {}, "ready")
            return process
        except CoreProtocolError:
            time.sleep(0.05)
    process.kill()
    process.wait()
    raise RuntimeError("core did not become ready")

def stop(process):
    process.send_signal(signal.SIGINT)
    try: process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()
        raise
    if process.returncode != 0: raise RuntimeError(process.stderr.read())
    process.stderr.close()

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    binary = parser.parse_args().binary.resolve()
    with tempfile.TemporaryDirectory(prefix="community-smoke-") as scratch:
        root = Path(scratch)
        data = root / "data"
        run(binary, "init", "--data-dir", data)
        registration = run(binary, "register", "--data-dir", data, "--id", "workspace", "--role", "app")
        token = next(line[6:] for line in registration.splitlines() if line.startswith("token="))
        process = start(binary, data)
        try:
            workspace = Workspace(CoreClient(str(data / "community.sock"), token), "garden")
            note = workspace.save_note(workspace.get("notes/welcome"), "Welcome", "Hello 🌱 café", command_id="create-note")
            stale = note
            note = workspace.save_note(note, "Welcome", "Hello 🌻 café", command_id="edit-note")
            assert note.state["body"] == "Hello 🌻 café"
            try:
                workspace.save_note(stale, "Welcome", "stale", command_id="stale")
                raise AssertionError("stale edit accepted")
            except CoreProtocolError as error: assert error.code == "REVISION_CONFLICT", error
            table = workspace.create_table("tables/supplies", "Supplies", {"item": "Item", "quantity": "Quantity"}, command_id="create-table")
            table = workspace.put_row(table, "seeds", {"item": "Seeds", "quantity": 12}, command_id="row-seeds")
            assert "Seeds,12" in table_csv(table)
            assert len(list(workspace.documents())) == 2
            before = workspace.changes()
            assert len(before["changes"]) == 4
            blocked = subprocess.run([str(binary), "register", "--data-dir", str(data), "--id", "bad", "--role", "app"], capture_output=True, text=True)
            assert blocked.returncode != 0 and "already in use" in blocked.stderr
            backup = root / "backup"
            run(binary, "backup", "--data-dir", data, "--destination", backup)
            run(binary, "verify-backup", "--source", backup)
        finally: stop(process)
        recovered = root / "recovered"
        run(binary, "restore", "--source", backup, "--data-dir", recovered)
        process = start(binary, recovered)
        try:
            workspace = Workspace(CoreClient(str(recovered / "community.sock"), token), "garden")
            assert workspace.get("notes/welcome").state == note.state
            assert workspace.get("tables/supplies").state == table.state
            assert workspace.changes(before["next_cursor"])["changes"] == []
        finally: stop(process)
    print("live workspace and recovery smoke: ok")

if __name__ == "__main__": main()
