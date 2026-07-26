# ADR 0004: Native toolkit module over authoritative operations and Loro

- Status: accepted
- Date: 2026-07-26

## Decision

Toolkit selection is a cohesive native modular-monolith slice, not a separate service, embedded research prototype, or universal plug-in framework. Domain vocabulary and deterministic validation live in `domain/toolkit`; use cases and query policy live in `application/toolkit`; fixed SQL and projection mechanics live in the existing rusqlite adapter.

Every toolkit write uses the existing staged-document and secure-log orchestration. One transaction on the sole SQLite writer commits the canonical signed encrypted p2panda operation, Loro update, log head, toolkit projection, outbox record, and idempotency response. No second database or connection owner is introduced.

Authoritative records use bounded sharded documents:

- `toolkit/concepts/<concept-key>/<revision>`;
- `toolkit/tools/<tool-key>`;
- `toolkit/assertions/<assertion-id>`;
- `toolkit/reviews/<review-id>`.

Accepted canonical operations and their Loro state are authoritative. Toolkit SQL tables are app/community-scoped, rebuildable query projections whose rows retain source operation hashes. Replication will exchange canonical operations, never SQLite files.

## Concept revisions

A concept's canonical normalized definition is hashed into an immutable revision. Any definition change creates a new revision and moves the active projection pointer; old revisions remain. Every assertion records the exact revision used for value, applicability, cardinality, enum, and assessment validation. Replicas therefore cannot silently reinterpret one signed assertion under different vocabulary.

New concepts are rows, not migrations. Domain, role, capability, standard, attribute, dimension, and descriptor remain distinct facets.

## Verification and conflict policy

Evidence, assertion values, assessment metadata, and reviews are separate records. Generic fact `confidence` is not reused as verification or assessment score. Unknown, missing, false, zero, proposed, rejected, and conflict remain distinct.

AI-origin assertions and AI-only evaluations begin proposed. An explicit review operation is required to verify them. Phase 1 authorization is only the local APP capability; group authorization is intentionally not claimed.

For cardinality-one concepts, queries aggregate eligible signed assertions independent of insertion order. More than one distinct verified value is an explicit conflict: all signed records are retained, no hard requirement is satisfied, and explanations identify the assertion IDs, revisions, values, evidence, and source operation hashes. A local command rejects a new conflicting verification already visible. Contradictory review records aggregate fail-closed rather than allowing last-inserted-wins behavior.

## Why

The research prototype established useful taxonomy, rubric, contract, safety, and SQLite findings, but its Python process and standalone database would violate the one-authority crash invariant and create the wrong replication artifact. Native integration reuses proven commit, staging, crypto, capability, and outbox paths while preserving inward dependencies.

SQLite is retained because fixed parameterized EAV-style queries support exact values, numeric bounds, explainability, and dynamic vocabulary. It is not authority: projections can be rebuilt from accepted operations/Loro documents. SQLite files are never copied, attached, merged, or synchronized. Device recovery uses operation replay and the stack's online backup path.

## Consequences and limits

- Logical export is deterministic, community-scoped, bounded, and excludes caches.
- Import is absent in this slice so it cannot bypass verification policy.
- Phase 1 produces local replicas and outbox records only.
- Inbound replication, group authorization, production group key management, and Reticulum operation transfer remain disabled gates.
- Future inbound projection must retain accepted concurrent assertions and reproduce the same conflict aggregation; it must never discard one based on arrival order.
