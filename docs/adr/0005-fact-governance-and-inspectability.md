# ADR 0005: Versioned claims, revisions, schemas, and inspection

- Status: accepted
- Date: 2026-07-26

## Decision

Facts are signed **claims**, not resolved truth. Accepted canonical p2panda records and checkpoints are authoritative; Loro documents are reconstructed from accepted records. `fact_claim_revisions`, `fact_claims`, search indexes, and future vectors are rebuildable projections.

Every governed claim references an immutable `(app_id, schema_id, schema_version)` registry entry. Predicates are namespaced and versioned as `<schema_id>@<version>`. The bounded schema subset has a JSON-object root, closed objects, bounded depth/node/property/array/string counts, no `$ref`, and no remote resolution. Registration is local ADMIN policy; APP and TRANSPORT credentials cannot modify it.

A claim write validates namespace, schema/predicate match, bounded object shape, confidence, provenance, lifecycle transition, and active-claim cardinality before Loro mutation, signing, or durable writes. One SQLite transaction then commits the canonical operation, Loro update, revision, current row, log head, outbox item, and idempotency response. Updating a claim appends a revision, links the prior operation, marks it `SUPERSEDED`, and advances the current row. Retraction/dispute/expiry are new signed revisions; accepted history is never hard-deleted.

`fact.inspect` links the current row, revision, operation status, author, provenance, schema, and Loro document. `fact.history` is cursor-paged and bounded. `system.doctor` and `projection.check` expose redacted consistency checks. Staging tables exist for future repair, but `projection.rebuild` remains explicitly disabled until canonical replay into staging and atomic validated replacement are complete.

## Authority and compatibility

Authority order is:

1. accepted signed records and accepted checkpoints;
2. Loro state reconstructed from those records;
3. fact revision/current/search/vector projections.

Migration 3 converts existing current fact rows into one `legacy/untyped` revision with schema version `0`; it does not invent a schema or source-document provenance and does not alter operation bytes or Loro updates. New `fact.assert` writes must use a registered enabled schema. Legacy rows remain queryable and inspectable.

APP inspection is restricted to the namespace derived from its local capability. Community membership authorization is not implemented: future p2panda-auth integration behind `GroupPolicy` must govern accepted remote writes and community reads.

## Consequences

Revision status rows are projection data and may be deterministically recomputed (for example, an older row becomes `SUPERSEDED`), but rows and accepted operations are never deleted. Schema registration is device-local administrative policy in this milestone and is not represented as a replicated group-authorized operation. Doctor output contains identifiers/hashes only, never object payloads, tokens, or keys. No sqlite-vec dependency or generic SQL administration endpoint is introduced.
