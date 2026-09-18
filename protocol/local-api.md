# Local API v1

## Framing

Each request and response is one frame: `u32 big-endian length || JSON bytes`. Connections may carry multiple sequential requests. Requests are limited to 1 MiB and responses to 4 MiB. A malformed frame closes the connection. Malformed JSON inside a bounded frame
returns `INVALID_JSON` with a redacted message; frame-length and I/O failures
close the connection without echoing parser or transport details.

Request envelope:

```json
{"v":1,"id":"caller-unique-id","token":"64 lowercase/uppercase hex chars","method":"document.get","params":{}}
```

Response envelope:

```json
{"v":1,"id":"caller-unique-id","ok":true,"result":{}}
```

or:

```json
{"v":1,"id":"caller-unique-id","ok":false,"error":{"code":"INVALID_REQUEST","message":"redacted-safe detail","retryable":false}}
```

The error object is a typed boundary, not a serialization of an internal
error string. `code` is stable machine-readable vocabulary, `message` is an
explicitly safe client message, and `retryable` is the server's bounded retry
hint. Internal causes remain server-side only. The v1 codes are:

- `UNAUTHENTICATED`
- `FORBIDDEN`
- `CONFLICT`
- `REVISION_CONFLICT`
- `IDEMPOTENCY_CONFLICT`
- `METHOD_NOT_FOUND`
- `INVALID_REQUEST`
- `INVALID_JSON`

Unexpected internal failures use the safe `INVALID_REQUEST` boundary for v1
compatibility; clients must not treat its message as diagnostic detail.

`health` does not require a token. Every other call does. Applications must generate a fresh, durable idempotency key for each intended write and reuse it after timeout/disconnection.

## `document.mutate` (APP)

Parameters:

```json
{
  "community_id":"community",
  "document_id":"document/shard",
  "idempotency_key":"uuid-or-ulid",
  "schema_version":1,
  "mutations":[
    {"op":"map_set","container":"meta","key":"title","value":"Welcome"},
    {"op":"map_delete","container":"meta","key":"draft"},
    {"op":"text_insert","container":"body","index":0,"text":"Hello"},
    {"op":"text_delete","container":"body","index":0,"length":2},
    {"op":"list_insert","container":"tags","index":0,"value":"local-first"},
    {"op":"list_delete","container":"tags","index":0,"length":1}
  ]
}
```

Primitive values are null, boolean, signed 64-bit integer, finite float, or string. A successful response includes operation hash, `APPLIED`, `durable: true`, and current JSON state. Same app + idempotency key + exact request returns the stored response. Reusing a key with another request returns `IDEMPOTENCY_CONFLICT`.

Indexes use Loro's container semantics. Frontends should use stable cursors in a later API for editing under concurrent remote changes; positional edits are the bounded Phase 1 contract.

## `document.get` (APP)

Parameters: `{ "community_id": "...", "document_id": "..." }`.

Returns the current deep JSON state, owning document key, and replayed update count.

## Fact schemas (ADMIN)

`schema.register` stores a device-local versioned policy for one application namespace. `schema.get` and `schema.list` are also ADMIN-only. APP and TRANSPORT tokens cannot administer schemas.

```json
{
  "app_id":"org.example.library",
  "schema_id":"org.example.library/located_at",
  "schema_version":1,
  "predicate":"org.example.library/located_at@1",
  "object_schema":{
    "type":"object",
    "properties":{"shelf":{"type":"string","maxLength":32}},
    "required":["shelf"],
    "additionalProperties":false
  },
  "multiple_active_claims":false,
  "provenance_policy":"SOURCE_AND_DOCUMENT",
  "lifecycle_policy":{"allow_supersession":true,"allow_retraction":true,"allow_dispute":false,"allow_expiry":false},
  "enabled":true
}
```

Schema IDs must be app-namespaced and predicates must equal `<schema_id>@<schema_version>`. The supported schema subset is intentionally closed: one root object, `type`, closed `properties`/`required`, bounded nested objects/arrays/strings, finite numeric bounds, and bounded string enums. Depth is at most four, nodes 256, properties 64 per object, array `maxItems` 128, and string `maxLength` 4096. Unknown keywords, `$ref`, remote resolution, and unbounded containers fail closed. Encoded schemas are at most 64 KiB. Provenance policies are `NONE`, `SOURCE`, `SOURCE_DOCUMENT`, and `SOURCE_AND_DOCUMENT`.

`schema.get` takes `app_id`, `schema_id`, and optional `schema_version` (latest when absent). `schema.list` takes `app_id` and `limit` (1–500). Registration records a redacted audit event but is local administrative policy, not a group-authorized replicated operation.

## `fact.assert` (APP)

Appends a claim revision and advances the current projection in the same transaction as the signed record, Loro update, log head, outbox, and idempotency response:

```json
{
  "community_id":"library",
  "claim_id":"claim-1",
  "subject":"book:123",
  "predicate":"org.example.library/located_at@1",
  "object":{"shelf":"A4"},
  "source":"inventory-2026",
  "source_document_id":"inventory/2026-07",
  "confidence":0.95,
  "schema_id":"org.example.library/located_at",
  "schema_version":1,
  "lifecycle_status":"ASSERTED",
  "idempotency_key":"unique-command-id"
}
```

Lifecycle values are `ASSERTED`, `RETRACTED`, `DISPUTED`, and `EXPIRED` for submitted revisions; prior current revisions become `SUPERSEDED`. A first revision must be asserted. `RETRACTED` and `EXPIRED` are terminal; further information uses a new claim ID. Policy controls other transitions. Retraction is a new signed revision and remains in history. The owning document is `facts/<claim_id>`. Object JSON is at most 64 KiB. Validation occurs before Loro mutation, signing, or writes. Exact idempotent retry returns the stored response without another operation/revision/update/outbox item.

Pre-migration facts remain queryable and inspectable as `legacy/untyped` schema version `0`; no misleading schema is inferred. New assertions require a registered enabled schema.

## `fact.query`, `fact.inspect`, and `fact.history` (APP)

`fact.query` reads active current claims from the rebuildable projection:

```json
{"community_id":"library","subject":"book:123","predicate":"org.example.library/located_at@1","limit":50}
```

Subject and predicate are optional exact filters; limit is capped at 500. Results include schema/lifecycle/provenance and the current revision operation hash. Claims are not resolved truth.

`fact.inspect` takes `{ "community_id":"library", "claim_id":"claim-1" }`. It returns the current claim and revision hash, author key, semantic provenance, schema, lifecycle, owning Loro document ID, revision count, operation verification/apply status, supersession links, and a projection consistency result.

`fact.history` takes the same identifiers plus `limit` (1–100, default 25) and an optional opaque cursor. It returns newest-first revisions and `next_cursor`; callers must not parse the cursor. History is always bounded and remains stable because its ordering tuple is `(asserted_at_ms, operation_hash)`. APP namespace comes only from the capability. Community authorization remains future `GroupPolicy` work.

Vector/semantic retrieval remains deferred until provenance/history/repair are mature; sqlite-vec is not included.

## Native profile discovery and grants

`profile.list` and `profile.get` are available to APP and ADMIN principals. `profile.get` accepts `profile_id`, optional `profile_version`, and optional `community_id`. Results return the byte-canonical manifest digest, shared data namespace, supported record contract, and temporal/extension policy. Access flags are populated only when a community is supplied; grants never apply globally.

`profile.grant` is ADMIN-only:

```json
{"app_id":"org.example.calendar","profile_id":"community.planning","community_id":"team","can_read":true,"can_write":true}
```

Write access requires read access. Grants target enabled APP principals, are scoped to exactly one community, and produce a redacted audit event. They prevent local cross-community access but are not replicated membership or p2panda-auth authorization.

## `community.planning@1` methods (APP)

All planning methods require an explicit grant for the requested profile and community. Writes are exact-idempotent and atomically commit a signed profile-bound Loro update, shared-namespace projection, log head, outbox row, and caller-scoped idempotency response.

- `planning.project.put`: project ID, title, optional description, and status.
- `planning.task.put`: task ID, optional existing project, title/description, status, optional start/due epoch milliseconds, and progress 0–100.
- `planning.event.put`: event ID, optional existing project, title/description, status, timing, optional bounded recurrence and location.
- `planning.dependency.put`: existing predecessor/successor task IDs, dependency kind, and bounded lag. Missing references and cycles fail closed.
- `planning.calendar.list`: bounded event view.
- `planning.gantt.get`: bounded project/task/dependency view.

Example timed event:

```json
{
  "community_id":"team", "idempotency_key":"event-1",
  "event_id":"standup", "project_id":"release", "title":"Stand-up",
  "status":"active",
  "timing":{"kind":"utc","start_at_ms":1786352400000,"end_at_ms":1786356000000,"time_zone":"Europe/Berlin"},
  "recurrence":{"frequency":"weekly","interval":1,"count":8,"by_weekday":["monday"]},
  "location":"Room 4"
}
```

All-day timing uses `kind:all_day`, `start_date`, and exclusive `end_date_exclusive`. Statuses are `planned`, `active`, `completed`, `cancelled`, and `archived`. Dependency kinds are `finish_to_start`, `start_to_start`, `finish_to_finish`, and `start_to_finish`. View requests contain `community_id` and limit 1–500.

The signed operation namespace is `community.planning`, not the caller's APP ID. The encrypted signed record also binds the canonical manifest digest and `submitted_by_app_id`; caller identity and shared data namespace are deliberately distinct. Generic document methods remain caller-private. See [`profiles/community-planning-v1.md`](profiles/community-planning-v1.md) for normative semantics and limitations.

## Native toolkit methods (APP)

Toolkit methods are APP-capability scoped: the application namespace always comes from the bearer capability. Every row is additionally scoped by `community_id`. Toolkit parameter objects reject unknown fields. All toolkit writes require `community_id` and `idempotency_key` and use the same atomic operation/update/head/projection/outbox/idempotency transaction as `document.mutate`.

### `toolkit.schema.list` / `toolkit.schema.show`

```json
{"community_id":"research","active_only":true,"limit":100}
```

`toolkit.schema.show` accepts `{"community_id":"research","key":"cap.sync.peer-to-peer","revision":null}`. An omitted/null revision resolves the active revision. Results include the immutable 64-hex-character revision and source operation hash.

### `toolkit.schema.add`

Concept fields are direct members beside `community_id` and `idempotency_key`:

```json
{
  "community_id":"research", "idempotency_key":"schema-1",
  "key":"cap.sync.peer-to-peer", "name":"Peer-to-peer exchange",
  "kind":"capability", "definition":"...", "aliases":["P2P"],
  "value_type":"boolean", "cardinality":"one",
  "applicable_categories":["role.file-sync"],
  "allowed_values":null, "scale":null, "parent":null, "status":"active"
}
```

Kinds are `domain`, `role`, `capability`, `standard`, `attribute`, `dimension`, and `descriptor`. Value types are `boolean`, `integer`, `number`, `text`, `enum`, `date`, and `reference`. Cardinality is `one` or `many`. Enum concepts require bounded allowed values. Dimensions require a numeric type and a scale containing finite `min`/`max`, `rubric`, `version`, and string-keyed `anchors`. Referenced parents/categories must exist. Arrays are sorted/deduplicated before canonical hashing. A changed definition creates a new immutable revision in `toolkit/concepts/<key>/<revision>`; assertions never change revision references.

### `toolkit.tool.add`

```json
{
  "community_id":"research", "idempotency_key":"tool-1",
  "key":"p2panda", "name":"p2panda", "description":"...",
  "homepage":"https://p2panda.org/", "status":"active"
}
```

Only `community_id`, `idempotency_key`, `key`, and `name` are required. The authoritative document is `toolkit/tools/<tool-key>`.

### `toolkit.assert`

```json
{
  "community_id":"research", "idempotency_key":"assert-1",
  "assertion_id":"assertion-1", "tool":"p2panda",
  "concept":"dimension.sync.transport-agnosticism", "concept_revision":null,
  "value":6, "origin":"human", "verification_state":"verified",
  "source":"https://...", "evidence":"...", "as_of":"2026-07-26",
  "rubric":"transport-agnosticism", "rubric_version":"1",
  "rationale":"...", "evaluator_type":"human", "evaluation_date":"2026-07-26"
}
```

An omitted revision binds the then-active revision and returns it in the authoritative assertion. States are `proposed`, `verified`, `rejected`, and `unknown`; explicit unknown omits/uses null `value`. Unknown, false, and zero are distinct. Origin is `human`, `ai`, or `fixture`. AI assertions and AI-only assessments must begin proposed. Dimension assertions require complete rubric metadata matching the concept scale. Non-dimensions reject assessment fields. Applicability, type, enum, scale, and local cardinality rules fail closed. The authoritative document is `toolkit/assertions/<assertion-id>`.

### `toolkit.verify`

```json
{
  "community_id":"research", "idempotency_key":"review-1",
  "review_id":"review-1", "assertion_id":"assertion-1",
  "state":"verified", "reviewer":"local-reviewer",
  "rationale":"Evidence checked.", "review_date":"2026-07-26"
}
```

Review state is `verified` or `rejected`. Reviews are immutable documents at `toolkit/reviews/<review-id>` and revalidate type, category applicability, and cardinality. The APP capability is the only local authorization available in Phase 1; group-authorized review is not yet implemented. Conflicting local verification is rejected. Future accepted concurrent signed values are retained and projected as conflict rather than discarded.

### `toolkit.query`

```json
{
  "community_id":"research", "tools":["p2panda","reticulum"],
  "include_partial":true,
  "requirements":[
    {"concept":"cap.sync.peer-to-peer","op":"eq","value":true,"required":true},
    {"concept":"dimension.sync.transport-agnosticism","op":"gte","value":8,"required":false}
  ]
}
```

`mandatory` and `optional` arrays are also accepted and normalize to requirements with `required` true/false. There must be 1–64 requirements. Operators are `eq`, `ne`, `in`, `exists`, `gt`, `gte`, `lt`, and `lte`; ranges are numeric only. Query SQL is fixed and parameterized—callers never submit SQL.

Only uniquely verified, non-conflicting assertions satisfy requirements. Positive operators on cardinality-many concepts are existential. `ne` requires verified values and succeeds only when every value differs. Hard requirements are conjunctive. Requirement states are `satisfied`, `unsatisfied`, `unknown`, `missing`, `proposed`, `rejected`, and `conflict`. False and zero are ordinary known values. Ranking uses only the submitted requirements, then tool key as a deterministic tie-break; unrelated assertions have no effect. Explanations include assertion IDs, exact concept revisions, values, evidence/source, assertion/review operation hashes where applicable, and all conflicting assertions.

### `toolkit.explain`

Accepts `community_id`, `tool`, and `query` (a query plan without `community_id`). It reruns the validated plan for that tool and returns its complete requirement explanation. No disposable explanation cache is required in this slice.

### `toolkit.export`

```json
{"community_id":"research","offset":0,"limit":500}
```

Returns deterministic key/ID-sorted pages (maximum 500 per logical collection) of concepts, tools, assertions, and reviews. It excludes caches and sets `import_supported:false`. Import is deliberately absent: recovery remains canonical operation replay plus community-stack online backup, and no restore path may bypass review policy. Never synchronize, copy, attach, or merge SQLite files between replicas.

## `system.doctor` and projections (ADMIN)

`system.doctor` takes an empty object and defaults to read-only mode. Every check returns `check_name`, `status` (`OK`, `WARNING`, or `ERROR`), bounded `count`, at most ten example identifiers, and `repairable`. Checks cover SQLite quick/integrity, foreign keys, operation/update references, author sequence/backlinks, bounded Loro replay, APPLIED/pending contradictions, current/revision links, broken/cyclic chains, current/latest disagreement, outbox/ACK orphans, and migration state. Output never includes object payloads, tokens, private keys, or plaintext credentials.

`projection.check` takes an empty object and deterministically compares fact current rows with latest revision-derived rows using counts and BLAKE3 row hashes. Mismatch examples contain only app/community/claim identifiers. `projection.rebuild` is present but returns `enabled:false` and never modifies data: staging tables exist, but safe canonical replay, staging validation, atomic replacement, and repair audit are deliberately deferred. Neither method rewrites canonical operations, signatures, Loro updates, credentials, checkpoints, or idempotency records.

## `outbox.claim` (TRANSPORT)

Parameters:

```json
{"transport":"reticulum","limit":16,"max_bytes":65536}
```

The transport value must equal the capability's registered principal ID. Returns operation hash, canonical header and ciphertext body as standard Base64, numeric priority, and a `lease_attempt` generation that must accompany the ACK. Claims are leased for 60 seconds. Limits are capped at 256 records; the byte budget excludes JSON/Base64 overhead and no oversized item is returned.

## `outbox.ack` (TRANSPORT)

```json
{
  "transport":"reticulum",
  "operation_hash":"64 hex chars",
  "lease_attempt":1,
  "status":"STORED",
  "detail":null
}
```

Statuses: `STORED`, `APPLIED`, `PENDING_DEPS`, `REJECTED`. This is an application ACK from the remote core, not a packet/Link/Resource receipt. ACK succeeds only for the matching current, unexpired `CLAIMED` lease generation; duplicate, expired, and stale post-reclaim ACKs fail.

## Compatibility policy

Additive optional fields may appear within v1 responses. Callers must ignore unknown response fields. Required-field, meaning, framing, or authorization changes require API v2. Binary CRDT/envelope formats have independent version fields and compatibility fixtures.
