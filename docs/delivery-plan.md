# Delivery plan and gates

## Phase 1 — local durable core (this repository)

Implemented baseline:

- versioned capability-scoped Unix IPC;
- Loro semantic mutation staging and replay;
- signed p2panda-core envelope over object ciphertext;
- single rusqlite writer and atomic operation/update/head/outbox/idempotency commit;
- transport outbox leases and durable ACK vocabulary;
- governed claim documents with ADMIN-registered bounded schemas, append-only signed revision history, atomic current projections, app-scoped inspection/history, structured doctor checks, and deterministic projection verification;
- ordered, checksummed migration runner with concurrent-start tests, tested v1/v3 upgrades, unknown-version rejection, and explicit `legacy/untyped` compatibility;
- native content-addressed `community.planning@1` interoperability profile with separate APP/data namespaces, ADMIN community-scoped profile grants, fixed project/task/event/dependency semantics, shared calendar/Gantt views, and pinned conformance artifact;
- native toolkit-selection module with immutable concept revisions, evidence/review records, fail-closed queries, sharded Loro authority, atomic rebuildable projections, bounded export, and fixture API loader;
- modular-monolith ports/adapters with CI-enforced dependency boundaries;
- pinned Reticulum sidecar identity/Link/Channel negotiation baseline.

Exit work:

- crash/failpoint matrix at every SQLite statement/commit boundary;
- property tests for idempotency and log-head compare-and-swap;
- implement and externally review the proposed encrypted backup format in [`adr/0007-encrypted-backup-design.md`](adr/0007-encrypted-backup-design.md), including encrypted full snapshots and replay-cost policy;
- online backup/restore command and corruption drills;
- enable fact projection rebuild only after canonical operation replay into staging, deterministic validation, atomic replacement, interruption tests, and redacted repair audit are complete;
- additional native profiles only after planning-profile conformance experience; avoid a universal validator/projector interface until extension, mapping, and profile-authority rules are proven;
- full RFC 5545 recurrence/exception/TZDB policy and a deterministic iCalendar adapter before any CalDAV adapter;
- wire fixture corpus checked into `tests/fixtures`.

Acceptance: after every injected crash, restart reconstructs exactly the last acknowledged state.

Phase 1 creates **local replicas and transport-independent outbox records only**. Fact projection rebuild remains deliberately disabled; staging and check infrastructure must not be mistaken for a repair command. Toolkit records are signed and encrypted for eventual transfer, but inbound synchronization, group authorization of reviews, production group key management, and Reticulum operation transfer remain disabled. SQLite files are never a replication artifact.

## Phase 2 — fast replication

- Implement the repository/log traits needed by low-level `p2panda-sync`.
- Add exact two-party sync over an in-memory transport first.
- Add optional iroh adapter without using high-level `p2panda::Node` or SQLx store.
- Implement the full inbound state machine, pending inbox, quarantine quotas, ACK persistence, and deterministic rebuild of toolkit conflict projections from accepted operations.
- Test duplicate/reordered/missing operations, three-way partitions, snapshot bootstrap, and restarts.

Acceptance: three replicas converge after arbitrary partitions/restarts, and no unauthorized fixture reaches Loro.

## Phase 3 — Reticulum operation transport

- Freeze MessagePack control IDs and cross-language fixtures.
- Implement paged manifests/range requests through `p2panda-sync` protocol ports.
- Channel: handshake, manifests, requests, small records, ACK, cancellation.
- Resource: explicitly offered/accepted bounded bundles/checkpoints only.
- Add exact reconciliation, retry leases, dependency requests, and airtime token buckets.
- Run shared `rnsd`, slow-link simulation, then real LoRa/packet-radio hardware.
- Enable `operation_transfer: true` only after all interop gates pass.

Acceptance: malicious input and long disconnections cannot make disk, memory, queues, retries, or airtime unbounded.

## Phase 4 — communities, identity, and encryption

- User root and per-device credentials; separate transport bindings.
- Integrate pinned `p2panda-auth` behind `GroupPolicy` for Pull/Read/Write/Manage at referenced frontiers, including remote shared-profile records and profile-version authority.
- Integrate reviewed group encryption behind `GroupCrypto`; separate spaces for real read boundaries.
- Strong-removal conflict policy, key epochs, rotation, historical-key policy, and rejected-offline-edit UX.
- Relay mode that stores/verifies ciphertext without content keys.
- External cryptographic/security review before removing the experimental flag.

Acceptance: model-based conflict tests prove deterministic authorization; removal prevents future decrypt/write while documenting unavoidable historical knowledge.

## Phase 5 — production operations

- quotas/abuse controls and redacted metrics;
- checkpoint attestations, multiple archival replicas, and safe pruning;
- content-addressed blob subsystem with radio-disabled default;
- migration tests from every supported release;
- CBOR, MessagePack, compression, Loro, SQLite, and sidecar IPC fuzzing;
- power-loss tests on deployment hardware;
- online backup automation and restore drills;
- dependency/license/SBOM checks and external audit.

## Upgrade policy

The compatibility baseline is:

- p2panda-core and p2panda-sync `0.7.0` (upstream tag commit `37be61875f285956c906002d74a2d0370df5b2c1`);
- Loro `1.13.7` from crates.io;
- rusqlite `0.40.1` (upstream tag commit `6d3c282dc5531a57eb4e22ece3207f00c95d0fb0`);
- Reticulum wheel `1.4.1`, SHA-256 pinned in requirements.

Cargo's exact requirements plus `Cargo.lock` checksums pin registry artifacts. Before any upgrade:

1. record source commit, artifact checksum, license, and security notes;
2. regenerate and compare canonical operation/control golden fixtures;
3. run database migration and downgrade/read-compat tests;
4. run Loro convergence/snapshot/shallow-boundary tests;
5. run Python/native Reticulum interoperability and hardware profiles;
6. make the upgrade an explicit reviewed commit—never a floating dependency update.
