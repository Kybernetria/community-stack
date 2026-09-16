"""Gateway contract and real loopback HTTP security regression tests."""
import copy
import http.client
import importlib.util
import json
from pathlib import Path
import threading
import unittest

spec = importlib.util.spec_from_file_location("community_hub", Path(__file__).resolve().parents[2] / "examples/community-hub/server.py")
hub = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hub)


class FakeCore:
    def __init__(self):
        self.calls = []
        self.conflict = False

    def call(self, method, params, request_id):
        self.calls.append((method, copy.deepcopy(params), request_id))
        if method == "document.mutate" and self.conflict:
            raise hub.CoreProtocolError("secret payload must not leak", code="REVISION_CONFLICT")
        return {"revision": 7, "state": {"meta": {"title": "Latest", "kind": "note"}, "body": "new"}}


class HubTests(unittest.TestCase):
    def setUp(self):
        self.core = FakeCore()
        self.server = hub.HubServer(0, self.core, "session-test")
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.addCleanup(self.stop)

    def stop(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

    def request(self, value=None, *, raw=None, headers=None, method="POST", path="/api"):
        body = json.dumps(value) if raw is None else raw
        base = {"Origin": f"http://127.0.0.1:{self.server.server_port}",
                "X-Community-Hub-Session": "session-test", "Content-Type": "application/json"}
        base.update(headers or {})
        conn = http.client.HTTPConnection("127.0.0.1", self.server.server_port, timeout=2)
        conn.request(method, path, body=body, headers=base)
        response = conn.getresponse()
        status, response_headers, data = response.status, dict(response.getheaders()), response.read()
        conn.close()
        return status, response_headers, data

    def note(self):
        return {"method": "note.save", "params": {"community_id": "garden", "document_id": "notes/welcome",
            "command_id": "durable-command", "snapshot": {"revision": 2, "state": {"body": "a🌱b", "meta": {"kind": "note", "old": True}}},
            "title": "Title", "body": "a🌻b", "metadata": {"tag": "garden"}}}

    def test_allowlist_rejects_admin_transport_and_generic_mutations(self):
        for method in ["profile.grant", "system.doctor", "outbox.claim", "document.mutate", "anything", []]:
            with self.subTest(method=method):
                status, _, _ = self.request({"method": method, "params": {}})
                self.assertEqual(status, 400)
        self.assertEqual(self.core.calls, [])

    def test_session_origin_and_host(self):
        for headers in [{"Origin": "https://evil.example"}, {"Origin": "null"},
                        {"X-Community-Hub-Session": "wrong"}, {"X-Community-Hub-Session": ""},
                        {"Host": "evil.example"}, {"Origin": ""}]:
            with self.subTest(headers=headers):
                self.assertEqual(self.request({"method": "health", "params": {}}, headers=headers)[0], 403)
        self.assertFalse(self.core.calls)
        self.assertEqual(self.request({"method": "health", "params": {}})[0], 200)

    def test_malformed_requests_are_safe_json(self):
        for raw in ['[]', 'null', '1', '{', '{"method":"health","params":[]}',
                    '{"method":"health","params":{},"token":"bad"}',
                    '{"method":"document.changes","params":{"community_id":"g","after":true}}',
                    '{"method":"document.changes","params":{"community_id":"g","after":NaN}}',
                    'x' * (hub.MAX_HTTP_BODY + 1)]:
            with self.subTest(raw=raw[:90]):
                status, _, data = self.request(raw=raw)
                self.assertEqual(status, 400)
                self.assertEqual(json.loads(data)["error"]["code"], "INVALID_REQUEST")
        self.assertFalse(self.core.calls)

    def test_exact_retry_uses_same_mutations_and_fresh_request_ids(self):
        note = self.note()
        self.assertEqual(self.request(note)[0], 200)
        self.assertEqual(self.request(note)[0], 200)
        first, second = self.core.calls
        self.assertEqual(first[:2], second[:2])
        self.assertNotEqual(first[2], second[2])
        self.assertEqual(first[0], "document.mutate")
        self.assertEqual(first[1]["expected_revision"], 2)
        self.assertIn({"op": "map_delete", "container": "meta", "key": "old"}, first[1]["mutations"])
        self.assertIn({"op": "text_delete", "container": "body", "index": 1, "length": 1}, first[1]["mutations"])

    def test_conflict_reloads_without_second_mutation(self):
        self.core.conflict = True
        status, _, data = self.request(self.note())
        self.assertEqual(status, 409)
        error = json.loads(data)["error"]
        self.assertEqual(error["code"], "REVISION_CONFLICT")
        self.assertEqual(error["latest"]["revision"], 7)
        self.assertNotIn(b"secret payload", data)
        self.assertEqual([call[0] for call in self.core.calls], ["document.mutate", "document.get"])

    def test_invalid_row_values_do_not_reach_core(self):
        for values in [[], {"missing": 1}, {"a": {}}, {"a": 2**64}]:
            request = {"method": "row.put", "params": {"community_id": "g", "document_id": "t", "command_id": "k",
                "snapshot": {"revision": 1, "state": {"meta": {"kind": "table"}, "columns": {"a": "A"}}},
                "row_id": "r", "values": values}}
            self.assertEqual(self.request(request)[0], 400)
        self.assertFalse(self.core.calls)

    def test_static_routes_security_headers_and_no_source_exposure(self):
        status, headers, data = self.request(method="GET", path="/")
        self.assertEqual(status, 200)
        self.assertIn(b"Community Hub", data)
        self.assertIn("frame-ancestors 'none'", headers["Content-Security-Policy"])
        self.assertEqual(headers["Cache-Control"], "no-store")
        self.assertEqual(headers["X-Content-Type-Options"], "nosniff")
        for path in ["/server.py", "/../README.md", "/.git/config"]:
            self.assertEqual(self.request(method="GET", path=path)[0], 404)
        self.assertEqual(self.request(method="GET", path="/", headers={"Host": "evil"})[0], 403)

    def test_planning_is_explicit_validated_and_idempotent(self):
        request = {"method": "planning.task.put", "params": {
            "community_id": "garden", "command_id": "plan-stable", "record_id": "task-1",
            "title": "Plant trees", "description": "", "status": "planned",
            "project_id": "", "progress_percent": 20}}
        self.assertEqual(self.request(request)[0], 200)
        method, params, _ = self.core.calls[-1]
        self.assertEqual(method, "planning.task.put")
        self.assertEqual(params["idempotency_key"], "plan-stable")
        self.assertEqual(params["task_id"], "task-1")
        self.assertIsNone(params["project_id"])
        request["params"]["progress_percent"] = 101
        self.assertEqual(self.request(request)[0], 400)
        self.assertEqual(len(self.core.calls), 1)

    def test_table_creation_row_delete_and_csv(self):
        request = {"method": "table.create", "params": {"community_id": "g", "document_id": "table",
            "command_id": "create", "title": "Supplies", "columns": {"item": "Item"}}}
        self.assertEqual(self.request(request)[0], 200)
        self.assertEqual(self.core.calls[-1][1]["expected_revision"], 0)
        state = {"meta": {"kind": "table"}, "columns": {"item": "Item"}, "rows": {"r": True},
                 "cells": {'["r","item"]': "soil"}}
        request = {"method": "row.delete", "params": {"community_id": "g", "document_id": "table",
            "command_id": "delete", "row_id": "r", "snapshot": {"revision": 3, "state": state}}}
        self.assertEqual(self.request(request)[0], 200)
        self.assertEqual(self.core.calls[-1][1]["mutations"], [
            {"op": "map_delete", "container": "rows", "key": "r"},
            {"op": "map_delete", "container": "cells", "key": '["r","item"]'}])
        self.core.call = lambda *args: {"revision": 3, "state": state}
        status, _, data = self.request({"method": "table.csv", "params": {"community_id": "g", "document_id": "table"}})
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(data)["result"]["csv"], "Item\r\nsoil\r\n")

    def test_disconnected_core_returns_safe_offline_error(self):
        self.server.core = hub.CoreClient('/nonexistent-community-hub-test.sock', 'private-token')
        status, _, data = self.request({"method": "health", "params": {}})
        self.assertEqual(status, 502)
        self.assertEqual(json.loads(data)["error"]["code"], "PROTOCOL_ERROR")
        self.assertNotIn(b'private-token', data)
        self.assertNotIn(b'/nonexistent', data)

    def test_core_error_is_redacted(self):
        def fail(*args):
            raise hub.CoreProtocolError("token=private", code="FORBIDDEN")
        self.core.call = fail
        status, _, data = self.request({"method": "health", "params": {}})
        self.assertEqual(status, 502)
        self.assertNotIn(b"token=private", data)


if __name__ == "__main__":
    unittest.main()
