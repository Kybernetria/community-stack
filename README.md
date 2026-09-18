# Community Stack

A device-local replicated backend foundation for many offline-first applications. It puts one stable, capability-scoped local API in front of:

- **p2panda-core 0.7.0** for canonical signed operation envelopes;
- **Loro 1.13.7** for convergent document state;
- **rusqlite / SQLite 0.40.1** for the only transactional authority;
- a replaceable **Reticulum 1.4.1 Python sidecar** (negotiation baseline included);
- later, an optional `p2panda-net`/iroh fast transport adapter.

This repository implements the **Phase 1 local durable vertical slice**, plus contracts and boundaries for later transports. It deliberately does not claim production-ready group authorization, key distribution, inbound replication, or Reticulum operation transfer.

## Invariants already enforced

1. A frontend receives `durable: true` only after SQLite commit succeeds.
2. Canonical p2panda header bytes and ciphertext are retained.
3. Operation, Loro update, log head, idempotency result, and outbox are committed atomically.
4. Loro edits happen in a staged document; failed persistence cannot mutate acknowledged state.
5. The native toolkit module stores immutable concept revisions, evidence-bearing assertions, and review records in sharded Loro documents while atomically maintaining rebuildable SQL query projections.
6. While serving, SQLite has one dedicated repository owner thread; offline `init`/`register` commands must run with the service stopped. The runtime uses WAL, `FULL` synchronous mode, foreign keys, a busy timeout, and short `BEGIN IMMEDIATE` writes.
7. Payloads are compressed only when useful, then XChaCha20-Poly1305 encrypted, then bound to a signed p2panda header.
8. APP, ADMIN, and TRANSPORT tokens are separate capabilities. Only ADMIN can register fact schemas or run doctor/projection checks; an app cannot drain the transport outbox.
9. Governed facts append schema-bound revisions and atomically advance a current-value projection. Retractions remain signed history; legacy facts are explicitly `legacy/untyped`.
10. The native `community.planning@1` profile separates APP identity from a shared content-addressed data namespace, allowing independently granted calendar and Gantt clients to use the same signed records.
11. IPC is local-only, mode `0600`, length-prefixed, versioned, bounded, and correlated by request ID.
12. p2panda, Loro, rusqlite, and Reticulum are exactly version-pinned. `Cargo.lock` and the Python wheel hash pin transitive artifacts.

## Quick start

```bash
cargo build --locked
cargo run -- init --data-dir ./data
cargo run -- register --data-dir ./data --id org.example.wiki --role app --token-file ./data/wiki.capability
cargo run -- register --data-dir ./data --id device-admin --role admin --token-file ./data/admin.capability
cargo run -- register --data-dir ./data --id reticulum --role transport --token-file ./data/reticulum.capability
RUST_LOG=community_stack=info cargo run -- serve --data-dir ./data
```

Run `init` and `register` only while `serve` is stopped. `register --token-file PATH` is the recommended issuance path: it atomically creates a new mode `0600` file, refuses existing or symlink targets, syncs the token, and does not print the bearer value. The legacy command without `--token-file` still prints a token once for compatibility; store it in an OS keyring, not source control or command history.

The API socket defaults to `./data/community.sock`. Frames are:

```text
4-byte unsigned big-endian JSON byte length | UTF-8 JSON
```

Example request body:

```json
{
  "v": 1,
  "id": "write-018f",
  "token": "<64 hex characters>",
  "method": "document.mutate",
  "params": {
    "community_id": "garden-club",
    "document_id": "wiki/welcome",
    "idempotency_key": "0190b8f0-unique-command-id",
    "schema_version": 1,
    "mutations": [
      {"op": "text_insert", "container": "body", "index": 0, "text": "Hello mesh"},
      {"op": "map_set", "container": "meta", "key": "published", "value": true}
    ]
  }
}
```

Methods:

| Principal | Method | Purpose |
|---|---|---|
| Public | `health` | Local readiness, SQLite quick check, author key |
| APP | `document.mutate` | Durable semantic Loro transaction, idempotent |
| APP | `document.get` | Reconstruct and query materialized document state |
| APP | `fact.assert` | Append a validated schema-bound claim revision and atomically advance current SQL state |
| APP | `fact.query` | Fetch active current claims from the rebuildable projection |
| APP | `fact.inspect` / `fact.history` | Inspect provenance/linkage and cursor-page immutable claim history |
| ADMIN | `schema.register/get/list` | Manage the bounded versioned fact-schema registry |
| ADMIN | `system.doctor` | Run structured read-only integrity and replay checks |
| ADMIN | `projection.check/rebuild` | Verify disposable fact projections; rebuild is explicitly disabled in this milestone |
| APP/ADMIN | `profile.list/get` | Discover pinned native profiles and community-scoped caller access |
| ADMIN | `profile.grant` | Grant one APP local read/write access to one profile and community |
| APP | `planning.*` | Write shared projects/tasks/events/dependencies and read calendar/Gantt views |
| APP | `toolkit.schema.*` | List/show/add immutable controlled-concept revisions |
| APP | `toolkit.tool.add` | Add a tool identity |
| APP | `toolkit.assert` / `toolkit.verify` | Add immutable evidence assertions and explicit reviews |
| APP | `toolkit.query` / `toolkit.explain` | Run validated, fail-closed selection plans |
| APP | `toolkit.export` | Read a deterministic bounded logical projection page |
| TRANSPORT | `outbox.claim` | Lease bounded canonical records for one adapter |
| TRANSPORT | `outbox.ack` | Record an application-level durable ACK |

See [`protocol/local-api.md`](protocol/local-api.md) for the full contract.

## Toolkit fixture loader

The bounded development loader submits ordinary native API commands; it never opens SQLite or writes projections directly:

```bash
COMMUNITY_APP_TOKEN='<app token>' python3 examples/toolkit-seed.py \
  --socket ./data/community.sock --community research
```

Fixtures under `tests/fixtures/toolkit/` are copied research snapshots. Their verification labels are **not timeless product certifications**; re-check cited evidence before a real selection.

## Minimal verification frontend

A dependency-free development Fact Console exercises health, fact assertion, SQL queries, idempotent retries, and reconstruction of the authoritative Loro document:

```bash
COMMUNITY_APP_TOKEN='<app token>' COMMUNITY_ADMIN_TOKEN='<separate admin token>' \
  python3 examples/fact-console/server.py --socket ./data/community.sock
# Open the one-time URL (including its #session-secret) printed by the gateway.
```

The browser never receives the APP or ADMIN token. The gateway keeps the capabilities separate, requires a fresh per-process browser session secret on every RPC, and uses strict method allowlists; the ADMIN panel is unavailable without a separate ADMIN token. It consumes only the versioned Unix-socket API and imports no backend code or database schema. Do not expose this development gateway on a network interface.

## Community workspace frontend

[Community Hub](examples/community-hub/README.md) is a dependency-free, loopback-only
browser workspace for revision-aware notes, tables with CSV export, paged activity,
and optional shared planning. It uses only the versioned Unix-socket API and keeps
APP credentials in the Python gateway. Its README includes setup and verification.

## Reticulum sidecar baseline

```bash
python3 -m venv .venv
.venv/bin/pip install --require-hashes -r rns-bridge/requirements.txt
# Store the transport token in ./data/reticulum.capability with mode 0600.
COMMUNITY_TRANSPORT_TOKEN_FILE='./data/reticulum.capability' \
  .venv/bin/python rns-bridge/rns_bridge.py \
  --core-socket ./data/community.sock --identity ./data/reticulum.rid
```

The bridge currently establishes an identity/destination, announces, accepts at most 32 encrypted Links with five-minute idle eviction, validates bounded Hello messages, queries negotiated Channel MDU, and negotiates protocol/profile limits. `operation_transfer: false` is intentional: manifests, Resources, inbound authorization, and durable ACK fixtures must pass the interoperability gates in [`docs/delivery-plan.md`](docs/delivery-plan.md) before data transfer is enabled.

## Architecture

The code is a modular monolith with enforced inward dependencies:

```text
domain ← ports ← application ← adapters ← main
```

Run `scripts/check-boundaries.sh` to reject infrastructure leakage into domain/use-case code.

- [`docs/architecture.md`](docs/architecture.md) — component plumbing, trust boundaries, and flows
- [`docs/delivery-plan.md`](docs/delivery-plan.md) — gated path from this vertical slice to replicated production
- [`docs/adr/0001-low-level-p2panda.md`](docs/adr/0001-low-level-p2panda.md)
- [`docs/adr/0002-single-sqlite-authority.md`](docs/adr/0002-single-sqlite-authority.md)
- [`docs/adr/0003-modular-monolith-boundaries.md`](docs/adr/0003-modular-monolith-boundaries.md)
- [`docs/adr/0004-native-toolkit-module.md`](docs/adr/0004-native-toolkit-module.md)
- [`docs/adr/0005-fact-governance-and-inspectability.md`](docs/adr/0005-fact-governance-and-inspectability.md)
- [`docs/adr/0006-native-interoperability-profiles.md`](docs/adr/0006-native-interoperability-profiles.md)
- [`docs/adr/0007-encrypted-backup-design.md`](docs/adr/0007-encrypted-backup-design.md) — proposed v2 backup design
- [`docs/dependency-security.md`](docs/dependency-security.md)
- [`protocol/profiles/community-planning-v1.md`](protocol/profiles/community-planning-v1.md)
- [`protocol/reticulum-sync.md`](protocol/reticulum-sync.md)

## Important limitations

- The device content master key is a Phase 1 local key, not group key management. Replace the `master_key` dependency with an audited `GroupCrypto` adapter before multi-device use.
- p2panda-auth and p2panda-encryption/spaces are not integrated yet. Inbound replication remains disabled rather than accepting unauthorised updates.
- Document reads currently replay all updates. Snapshot/checkpoint selection is the next storage milestone.
- Fact projection rebuild is deliberately disabled. `projection.check` and staging schemas are present, but no repair command will run until canonical record replay and atomic staging validation are complete.
- Existing pre-migration facts remain `legacy/untyped` and queryable; new assertions require an enabled registered schema.
- Planning profile grants are device-local and community-scoped, but do not provide replicated group authorization. The v1 recurrence model is bounded rather than full RFC 5545, IANA-style zone names are syntax-checked without a pinned TZDB, and no CalDAV adapter exists yet.
- Toolkit validation/projection is a deliberately narrow native module seam, not a universal plug-in interface. Other application-defined validators still need a versioned module contract, especially for scarce-resource invariants.
- Toolkit import is not implemented. Recovery is canonical operation replay plus community-stack online backup; an export cannot bypass review policy by being restored directly. Current v1 device backups are plaintext at rest; the versioned encrypted-backup design is proposed in [`docs/adr/0007-encrypted-backup-design.md`](docs/adr/0007-encrypted-backup-design.md) and is not yet implemented.
- The Reticulum custom license must be reviewed for the intended distribution model.

Do not use this initial slice for payments, scarce inventory, safety-critical data, or high-risk confidential communications.
