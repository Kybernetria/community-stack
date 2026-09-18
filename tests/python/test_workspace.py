import copy
import json
from pathlib import Path
import struct
import sys
import unittest
from unittest.mock import patch
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "clients" / "python"))
from community_stack_ipc import CoreClient, CoreProtocolError
from community_workspace import Document, Workspace, table_csv

class CapturingClient:
    def call(self, method, params, request_id):
        self.request = copy.deepcopy(params)
        return {"revision": params["expected_revision"] + 1, "state": {}}

class WorkspaceTests(unittest.TestCase):
    def setUp(self):
        self.client = CapturingClient()
        self.workspace = Workspace(self.client, "garden")

    def test_unicode_note_diff_and_exact_retry(self):
        note = Document("garden", "note", 9, {"meta": {"kind": "note"}, "body": "a🌱b café"})
        self.workspace.save_note(note, "Title", "a🌻b café", command_id="stable")
        request = self.client.request
        self.assertEqual(request["expected_revision"], 9)
        self.assertEqual(request["mutations"][-2:], [
            {"op": "text_delete", "container": "body", "index": 1, "length": 1},
            {"op": "text_insert", "container": "body", "index": 1, "text": "🌻"}])
        self.workspace.save_note(note, "Title", "a🌻b café", command_id="stable")
        self.assertEqual(request, self.client.request)

    def test_cells_have_stable_unambiguous_ids(self):
        doc = Document("garden", "sheet", 1, {"meta": {"kind": "table"}, "columns": {"a/b": "Label"}})
        self.workspace.put_row(doc, "row/1", {"a/b": "=SUM(A1:A2)"}, command_id="cell")
        cell = self.client.request["mutations"][-1]
        self.assertEqual(json.loads(cell["key"]), ["row/1", "a/b"])
        self.assertEqual(cell["value"], "=SUM(A1:A2)")
        for values in [{"missing": 1}, {"a/b": float("nan")}, {"a/b": 2**64}]:
            with self.assertRaises(ValueError):
                self.workspace.put_row(doc, "r", values, command_id="bad")
        self.workspace.delete_row(doc, "row/1", command_id="delete")
        self.assertEqual(self.client.request["mutations"][-1]["key"], cell["key"])

    def test_csv_is_deterministic_and_quoted(self):
        doc = Document("garden", "sheet", 2, {"meta": {"kind": "table"},
            "columns": {"b": "Count", "a": "Item"}, "rows": {"r2": True, "r1": True},
            "cells": {'["r1","a"]': "seed, green", '["r1","b"]': 2, '["r2","a"]': "soil"}})
        self.assertEqual(table_csv(doc), 'Item,Count\r\n"seed, green",2\r\nsoil,\r\n')

    def test_csv_formula_like_text_is_neutralized_but_numbers_stay_typed(self):
        doc = Document("garden", "sheet", 2, {"meta": {"kind": "table"},
            "columns": {"a": " =heading", "b": "Number"}, "rows": {"r": True},
            "cells": {
                '["r","a"]': "  =SUM(A1:A2)",
                '["r","b"]': -42,
            }})
        self.assertEqual(table_csv(doc), "' =heading,Number\r\n'  =SUM(A1:A2),-42\r\n")
        self.assertEqual(doc.state["cells"]['["r","a"]'], "  =SUM(A1:A2)")

    def test_csv_formula_markers_cover_all_supported_prefixes(self):
        doc = Document("garden", "sheet", 2, {"meta": {"kind": "table"},
            "columns": {"a": "Label"}, "rows": {"r": True},
            "cells": {
                '["r","a"]': "@cmd",
            }})
        for marker in ("=", "+", "-", "@"):
            doc.state["cells"]['["r","a"]'] = marker + "1"
            self.assertTrue(table_csv(doc).startswith("Label\r\n'" + marker))

    def test_cross_community_snapshot_is_rejected(self):
        with self.assertRaises(ValueError):
            self.workspace.mutate(Document("private", "doc", 1, {}), [], command_id="bad")

class ProtocolTests(unittest.TestCase):
    def exchange(self, response):
        body = json.dumps(response).encode()
        class FragmentedStream:
            def __init__(self): self.buffer = bytearray(struct.pack(">I", len(body)) + body)
            def __enter__(self): return self
            def __exit__(self, *args): pass
            def settimeout(self, timeout): pass
            def connect(self, path): pass
            def sendall(self, request):
                assert len(request[4:]) == struct.unpack(">I", request[:4])[0]
            def recv(self, size):
                fragment = self.buffer[:1]
                del self.buffer[:1]
                return bytes(fragment)
        with patch("community_stack_ipc.socket.socket", return_value=FragmentedStream()):
            return CoreClient("/unused.sock", "00"*32).call("document.get", {}, "r")

    def test_fragmented_frames(self):
        self.assertEqual(self.exchange({"v": 1, "id": "r", "ok": True, "result": {"revision": 1}}), {"revision": 1})

    def test_typed_conflict(self):
        with self.assertRaises(CoreProtocolError) as caught:
            self.exchange({"v": 1, "id": "r", "ok": False, "error": {"code": "REVISION_CONFLICT", "message": "stale", "retryable": False}})
        self.assertEqual(caught.exception.code, "REVISION_CONFLICT")
        self.assertFalse(caught.exception.retryable)

    def test_malformed_shapes_and_correlation_fail_closed(self):
        for response in [[], {"v": 1, "id": "wrong", "ok": True, "result": 1},
                         {"v": 1, "id": "r", "ok": "true"}, {"v": 1, "id": "r", "ok": True},
                         {"v": 1, "id": "r", "ok": False, "error": []}]:
            with self.subTest(response=response), self.assertRaises(CoreProtocolError):
                self.exchange(response)

if __name__ == "__main__": unittest.main()
