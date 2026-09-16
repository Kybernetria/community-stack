"""Local note/table clients; persist command IDs and exact requests before sending."""
from __future__ import annotations
import csv
import io
import json
import math
import unicodedata
import uuid
from dataclasses import dataclass
from typing import Any
from community_stack_ipc import CoreClient

@dataclass(frozen=True)
class Document:
    community_id: str
    document_id: str
    revision: int
    state: dict[str, Any]

@dataclass(frozen=True)
class Workspace:
    client: CoreClient
    community_id: str

    def call(self, method: str, **params: Any) -> Any:
        return self.client.call(method, {"community_id": self.community_id, **params}, uuid.uuid4().hex)

    def get(self, document_id: str) -> Document:
        result = self.call("document.get", document_id=document_id)
        return Document(self.community_id, document_id, result["revision"], result["state"])

    def documents(self, prefix: str = ""):
        after = None
        while True:
            page = self.call("document.list", prefix=prefix, after=after, limit=100)
            yield from page["documents"]
            after = page["next_cursor"]
            if after is None:
                return

    def changes(self, after: int = 0, limit: int = 100, profile_id: str | None = None) -> dict[str, Any]:
        return self.call("document.changes", after=after, limit=limit, profile_id=profile_id)

    def mutate(self, document: Document, mutations: list[dict[str, Any]], *, command_id: str) -> Document:
        if document.community_id != self.community_id:
            raise ValueError("document belongs to a different community")
        result = self.call("document.mutate", document_id=document.document_id,
                           expected_revision=document.revision, idempotency_key=command_id, mutations=mutations)
        return Document(self.community_id, document.document_id, result["revision"], result["state"])

    def save_note(self, document: Document, title: str, text: str, *, command_id: str) -> Document:
        if document.state.get("meta", {}).get("kind") not in (None, "note"):
            raise ValueError("document is not a note")
        old = document.state.get("body", "")
        prefix = 0
        while prefix < min(len(old), len(text)) and old[prefix] == text[prefix]:
            prefix += 1
        suffix = 0
        while suffix < min(len(old), len(text)) - prefix and old[-suffix-1] == text[-suffix-1]:
            suffix += 1
        mutations = [_set("meta", "kind", "note"), _set("meta", "title", title)]
        removed = len(old) - prefix - suffix
        inserted = text[prefix:len(text)-suffix if suffix else len(text)]
        if removed:
            mutations.append({"op": "text_delete", "container": "body", "index": prefix, "length": removed})
        if inserted:
            mutations.append({"op": "text_insert", "container": "body", "index": prefix, "text": inserted})
        return self.mutate(document, mutations, command_id=command_id)

    def create_table(self, document_id: str, title: str, columns: dict[str, str], *, command_id: str) -> Document:
        if not columns or len(columns) > 128:
            raise ValueError("a table requires 1..128 columns")
        for column in columns:
            _identifier(column)
        mutations = [_set("meta", "kind", "table"), _set("meta", "title", title)]
        mutations.extend(_set("columns", column, label) for column, label in columns.items())
        return self.mutate(Document(self.community_id, document_id, 0, {}), mutations, command_id=command_id)

    def put_row(self, document: Document, row_id: str, values: dict[str, Any], *, command_id: str) -> Document:
        _identifier(row_id)
        _table(document)
        if not values.keys() <= document.state.get("columns", {}).keys():
            raise ValueError("row contains an unknown column ID")
        mutations = [_set("rows", row_id, True)]
        for column, value in values.items():
            if not (value is None or type(value) in (str, bool, int, float)):
                raise ValueError("cell values must be JSON primitives")
            if type(value) is int and not -(2**63) <= value < 2**63:
                raise ValueError("integer cell exceeds signed 64-bit range")
            if type(value) is float and not math.isfinite(value):
                raise ValueError("cell numbers must be finite")
            mutations.append(_set("cells", _cell(row_id, column), value))
        return self.mutate(document, mutations, command_id=command_id)

    def delete_row(self, document: Document, row_id: str, *, command_id: str) -> Document:
        _identifier(row_id)
        _table(document)
        mutations = [{"op": "map_delete", "container": "rows", "key": row_id}]
        for column in document.state.get("columns", {}):
            mutations.append({"op": "map_delete", "container": "cells", "key": _cell(row_id, column)})
        return self.mutate(document, mutations, command_id=command_id)


def table_csv(document: Document) -> str:
    """Lossy deterministic export, not backup; formula-looking strings remain literal."""
    _table(document)
    columns = sorted(document.state.get("columns", {}))
    output = io.StringIO(newline="")
    writer = csv.writer(output)
    writer.writerow([document.state["columns"][column] for column in columns])
    cells = document.state.get("cells", {})
    for row in sorted(document.state.get("rows", {})):
        writer.writerow([cells.get(_cell(row, column), "") for column in columns])
    return output.getvalue()


def _table(document: Document) -> None:
    if document.state.get("meta", {}).get("kind") != "table":
        raise ValueError("document is not a table")


def _identifier(value: str) -> None:
    if not isinstance(value, str) or not value or len(value.encode("utf-8")) > 64 or any(unicodedata.category(c) == "Cc" for c in value):
        raise ValueError("row/column IDs require 1..64 UTF-8 bytes without control characters")


def _cell(row: str, column: str) -> str:
    key = json.dumps([row, column], ensure_ascii=False, separators=(",", ":"))
    if len(key.encode("utf-8")) > 256:
        raise ValueError("encoded cell key exceeds 256 bytes")
    return key


def _set(container: str, key: str, value: Any) -> dict[str, Any]:
    return {"op": "map_set", "container": container, "key": key, "value": value}
