# Architecture

## Shape of the system

```text
 desktop / mobile / CLI / webview
       │  v1 bounded local IPC (APP capability)
       ▼
 ┌────────────────── community-stack Rust process ───────────────────┐
 │ API router → app boundary → keyed document actor                  │
 │                           │                                       │
 │                  staged Loro transaction                          │
 │                           │ update                                │
 │ group policy ─→ compress → object encrypt → p2panda-core sign     │
 │                           │                                       │
 │               one SQLite writer / one ACID commit                 │
 │          operation + update + head + projection + outbox + idem   │
 │                           │                                       │
 │                  transport-independent outbox                     │
 └───────────────────────────┬───────────────────────────────────────┘
       TRANSPORT capability  │ canonical header + ciphertext only
                 ┌───────────┴───────────┐
                 ▼                       ▼
       p2panda-sync adapter       protected Unix IPC
       optional iroh/fast         Python RNS bridge → shared rnsd
                                  Link/Channel/Resource → carriers
```

The process is a **replica**, not a server of record. Every full node owns state and can acknowledge local writes while disconnected. Hubs improve availability but have no special authority.

## Cohesive modular-monolith boundaries

The implementation uses ports and adapters without introducing services or distributed transactions:

```text
src/domain/             documents, facts, profiles/planning, toolkit, replication, security data
       ▲
src/ports.rs            Repository, DocumentEngine, ContentHasher, SecureLog
       ▲
src/application.rs      use cases and cross-domain transaction policy
       ▲
src/adapters/           SQLite, Loro, p2panda, local IPC
       ▲
src/main.rs             composition root only
```

Dependencies point inward. Domain values contain no engine-native types. The application layer has no imports from Loro, p2panda, rusqlite, cryptographic libraries, or concrete adapters. Adapters never invoke each other: the application coordinates them through `Repository`, `DocumentEngine`, `ContentHasher`, and `SecureLog`. `scripts/check-boundaries.sh` enforces these rules in CI.

Group policy and production group crypto remain explicit future ports. APP namespace scoping is implemented, but community read/write authorization is explicitly future `GroupPolicy` work and no p2panda-auth claim is made. They will be injected into the application rather than imported by document, database, API, or transport adapters. See ADR 0003.

## Governed claim path

A fact is a schema-bound **claim**, not resolved truth. Accepted signed records/checkpoints are authoritative, Loro state is reconstructed from them, and both revision/current SQL tables are disposable:

```text
schema.register (ADMIN)
  → bounded closed JSON-object schema + provenance/lifecycle policy
  → versioned local registry entry and redacted audit event

fact.assert (APP)
  → application loads the app-scoped enabled schema
  → validates subject, predicate/version, object, confidence, provenance,
    lifecycle transition, and active-claim cardinality before side effects
  → DocumentEngine stages `facts/<claim_id>`
  → SecureLog encrypts/signs the Loro update
  → one commit rechecks current-revision and single-active cardinality preconditions,
    appends fact_claim_revisions, supersedes the prior revision, advances fact_claims,
    and stores operation + update + head + outbox + idempotency

fact.inspect / fact.history
  → app-scoped linkage and bounded stable-cursor history
fact.query
  → active current claims without reconstructing Loro
```

Retraction, dispute, expiry, and supersession are represented by new signed operations. Accepted history is never hard-deleted. Migration 3 labels old projected facts `legacy/untyped@0`; new writes must reference registered schemas. `system.doctor` coordinates SQLite checks with Loro replay through ports, while `projection.check` compares deterministic counts and row hashes. Rebuild staging exists, but repair remains disabled until canonical replay and atomic validated replacement are complete. Future sqlite-vec rows follow the same authority/provenance rule and are not included yet.

## Native interoperability profile path

Database conventions are not the interoperability contract. Applications may use different UI/view models, while signed records bind one content-addressed semantic profile:

```text
ADMIN profile.grant(calendar APP, community.planning, team, read/write)
ADMIN profile.grant(gantt APP, community.planning, team, read/write)

calendar planning.event.put
  → validate native community.planning@1 rules
  → signed record binds profile ID/version/digest + submitting APP
  → operation/Loro document use shared `community.planning` namespace
  → caller-scoped idempotency + shared disposable planning projection

gantt planning.gantt.get
calendar planning.calendar.list
  → different bounded views over the same authoritative records
```

Caller identity and data namespace are separate. Code-owned profile namespaces cannot be registered as principal IDs, and generic document APIs defensively reject a reserved namespace; explicit planning use cases require a local grant for the exact community. This avoids sharing APP tokens or exposing generic cross-app reads. The byte-canonical profile manifest and digest are pinned under `protocol/profiles/`, and changing meaning requires a new profile version.

The first profile fixes projects, tasks, events, dependencies, temporal rules, recurrence bounds, and cycle policy. Application validation passes an observed community generation into the SQLite commit; `BEGIN IMMEDIATE` rechecks it before projection changes, so independent local cores cannot commit incompatible cross-record decisions from one stale view. It does not claim full iCalendar/CalDAV compatibility. Local profile grants and `submitted_by_app_id` are device-level attribution, not group authorization or independent cryptographic app authorship. Future inbound profile acceptance belongs behind `GroupPolicy`.

## Native toolkit path

Toolkit selection is a cohesive application module (`domain/toolkit.rs`, `application/toolkit.rs`, and `adapters/sqlite/toolkit.rs`), not a service or generic plug-in framework:

```text
toolkit schema/tool/assert/verify write
  → deterministic domain validation and immutable record construction
  → one sharded Loro document per concept revision, tool, assertion, or review
  → SecureLog encryption/signing
  → the existing atomic operation + update + head + toolkit projection + outbox + idempotency commit

toolkit query/explain/export
  → fixed parameterized reads from app/community-scoped rebuildable projections
  → application evaluation of verification, cardinality, applicability, and query-plan semantics
```

Concept definitions have canonical revision hashes. Assertions bind the exact revision used for validation, so replicas cannot silently reinterpret old evidence after vocabulary changes. Evidence, assessment metadata, and review state are separate from generic fact confidence.

AI assertions and AI-only assessments begin proposed. Only explicit review records can verify them. Cardinality-one queries aggregate all eligible signed assertions: distinct concurrent verified values become `conflict`, satisfy no hard requirement, and remain visible in explanations. Local writes reject a conflicting verification already visible. This is insertion-order independent and is designed for future accepted replicated operations; inbound replication and group-authorized review are not implemented in Phase 1.

Toolkit SQL rows retain source operation hashes and are disposable. New vocabulary is data, not a migration. Logical export is deterministic and bounded, excludes caches, and is not an import path. Recovery remains operation replay plus online SQLite backup. SQLite files are never copied, attached, merged, or synchronized between replicas.

## Record construction

A document log ID is a domain-separated BLAKE3 digest over length-delimited `(app, community, document)`. p2panda log continuity is per `(author key, log ID, generation)`.

```text
Loro bounded update
  → zstd level 3 only when it saves at least 32 bytes
  → CBOR payload { document_id, update, semantic transaction }
  → XChaCha20-Poly1305 using community/epoch key and contextual AAD
  → p2panda body (ciphertext)
  → p2panda canonical CBOR header containing body hash and extensions
  → Ed25519 author signature
```

Encryption AAD binds app/community/document metadata, author, sequence, and backlink. The signed body hash binds the exact ciphertext. Reticulum Link encryption is additional hop/session protection, never object-level protection.

Current extension fields:

- stack protocol version;
- app, community, and document identifiers;
- opaque community topic and deterministic document log ID;
- shard and record kind;
- app schema, Loro encoding, and compression codec versions;
- auth frontier and key epoch.

Before radio deployment, replace clear app/community/document extension strings with a privacy-reviewed opaque routing scheme. The current fields optimize Phase 1 inspection, not metadata privacy.

## Local write state machine

1. Authenticate APP capability; app identity comes from the capability, never request parameters.
2. Validate IDs, command bounds, and idempotency key.
3. Acquire a keyed document lock and recheck idempotency.
4. Reconstruct the last durable Loro state.
5. Fork a staging document and apply one semantic mutation batch.
6. Export only updates newer than the pre-edit version vector.
7. Read the current p2panda log head.
8. Compress, encrypt, sign, and self-validate the operation.
9. Enter `BEGIN IMMEDIATE`; compare the expected log head again.
10. Atomically insert operation/update, advance head, enqueue outbox, and store the exact response.
11. Commit, then return `durable: true`.

A crash before step 10 has no durable effect. A crash during step 10 is resolved by SQLite atomic recovery. A lost response after step 10 is replayed exactly by idempotency key.

## Required inbound state machine (next phase)

```text
frame limits → canonical decode → body size/hash → signature → dedupe
→ backlink/sequence → device credential → auth at referenced frontier
→ decrypt → bounded decompress → schema/domain validation
→ import into staging Loro doc → inspect pending dependencies
→ one SQLite commit → cache swap → durable application ACK
```

Records that arrive before a backlink/auth/key/causal dependency go to `durable_inbox` with a reason code. Invalid or unauthorized records go to `quarantine` under quotas. Neither affects live Loro state.

Transport receipt, Reticulum Resource completion, and p2panda sync `Done` are never durability ACKs.

## API entrypoint and isolation

The Linux Unix socket is the only frontend/adapter entrypoint in Phase 1:

- mode `0600` and no TCP listener;
- 32-byte random bearer capabilities stored only as BLAKE3 hashes;
- APP, ADMIN, and TRANSPORT roles are disjoint;
- schema/doctor/projection administration requires a separate ADMIN token;
- application namespace is derived from the APP token;
- transport namespace is derived from the TRANSPORT token;
- 1 MiB request and 4 MiB response bounds, with at most 128 concurrent socket handlers;
- 4-byte big-endian frame length, API version, request ID, typed error;
- secrets and payloads are excluded from logs;
- the optional loopback development gateway requires a fresh per-process browser session secret in addition to strict Host/Origin checks, and never sends APP/ADMIN tokens to the browser.

For multi-user appliances, split app and transport sockets by Unix group and add OS peer-credential checks. For sandboxed mobile applications, expose the same contract through a separately reviewed platform IPC/FFI adapter; do not run the Linux daemon through loopback TCP by default. See [`platform-support.md`](platform-support.md).

## SQLite ownership

While `serve` is running, one dedicated thread owns the repository's sole write-capable rusqlite connection. `init` and `register` are offline administrative commands and must be run while the service is stopped. No SQLx database or p2panda high-level Node is allowed beside it. Configuration:

- WAL;
- foreign keys on;
- five-second busy timeout;
- `synchronous=FULL`;
- short `BEGIN IMMEDIATE` commits;
- STRICT tables and explicit constraints.

Authority order:

1. accepted signed operations and accepted checkpoints;
2. Loro state rebuilt from those records;
3. disposable revision/current/search/vector projections, including fact, planning, and toolkit tables.

Migrations are ordered and recorded in `schema_migrations`; the applied-state check occurs inside each short `BEGIN IMMEDIATE` transaction. Version 3 adds fact governance, version 4 adds planning projections, and version 5 adds migration checksums, globally unique local principals, community-scoped profile grants, and planning generation CAS rows. Embedded migration bytes are checked against reviewed, test-pinned digests before any migration runs; startup also fails on unknown versions, stored checksum mismatch, missing legacy schema landmarks, or ambiguous legacy APP/ADMIN credentials. None deletes or rewrites operations, signatures, ciphertext, Loro updates, credentials, or checkpoints; upgrading to version 5 safely revokes old unscoped planning grants for explicit re-granting.

Backups must use SQLite online backup APIs. Never copy a live database file without its WAL protocol, and never synchronize SQLite files between devices. Future replication exchanges canonical operations only.

## Identity separation

Never equate these values:

| Identity | Ownership/use |
|---|---|
| User root | person/profile and device enrollment |
| Device credential | binds one enrolled device and capabilities |
| p2panda author key | signs application records; `author.ed25519` today |
| Reticulum identity | owns sync destination; Python sidecar file |
| iroh identity | optional fast transport authentication |
| Loro PeerID | random process writer session, non-cryptographic |
| Membership role | authorization in one community/auth DAG |
| Local API token | least-privilege local process capability |

The Phase 1 content master key is also separate from all identities. Production group keys belong behind `GroupCrypto` and need epoch rotation, history policy, and secure enrollment.

## Transport policy

All transports carry identical canonical records and converge through one ingest pipeline.

- Fast adapter: exact p2panda log sync over iroh/LAN/Internet; gossip only hints.
- Reticulum adapter: pull-based manifests/ranges; Channel controls; Resource only for accepted bounded bundles/checkpoints.
- Deduplication: p2panda operation hash.
- Scheduling: control/revocation, ACK/dependency, compact messages, updates, snapshots, blobs.
- Quotas: interface × community × peer × traffic class.

Do not tunnel QUIC through Reticulum, run normal p2panda gossip over LoRa, or infer completeness from Bloom filters.

## Failure containment

| Failure | Containment |
|---|---|
| Frontend retries | atomic idempotency response |
| Process crash during write | SQLite transaction + staged Loro state |
| Duplicate transport delivery | operation hash primary key |
| Reordered author log | durable pending inbox until backlink exists |
| Missing Loro causes | `PENDING_DEPS`, never falsely `APPLIED` |
| Decompression bomb | compressed and expanded byte limits |
| Sidecar compromise | no p2panda signing key; opaque transport records only |
| Transport key compromise | does not grant operation authorship |
| Projection bug | rebuild from accepted operations/checkpoint |
| Radio outage | bounded durable queue and pull scheduling |

## Scaling many applications

Applications get separate capability principals and namespaces. Communities/documents are shards; never put a whole community into one Loro document. Add application modules as explicit versioned validators/projectors, not transport plugins. The toolkit module is the first narrow native example; it is not a premature universal framework. Each future module must declare:

- accepted command and schema versions;
- maximum command/update/state sizes;
- document/shard strategy;
- deterministic validation rules;
- projection migrations and rebuild procedure;
- whether domain invariants require escrow, reservation, quorum, or conflict UX beyond CRDT convergence.
