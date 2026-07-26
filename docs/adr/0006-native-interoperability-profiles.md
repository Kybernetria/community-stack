# ADR 0006: Native interoperability profiles and shared data namespaces

- Status: accepted
- Date: 2026-07-26

## Context

Replicas can converge technically while applications disagree semantically. Standardizing SQLite tables would not solve this: SQL tables are local projections, just as interoperable CalDAV servers may use unrelated databases. Interoperability must bind shared meaning to signed records.

Facts currently provide app-local schema governance. Their locally registered schemas are intentionally insufficient as cross-device profile identities because two administrators could register different definitions under one app schema ID/version.

## Decision

Introduce concrete, code-owned native interoperability profiles before attempting a universal profile registry. The first is `community.planning@1`.

A native profile has a byte-canonical manifest, pinned content digest, versioned API commands, deterministic validation, document identity rules, conformance fixtures, and rebuildable projections. Signed profile records bind profile ID, version, manifest digest, and submitting local APP.

Application principal identity is distinct from data namespace. Planning records are stored under the shared `community.planning` namespace, while idempotency remains scoped to the calling APP. A local ADMIN grants each APP explicit read or read/write access for one profile and community. Code-owned data namespaces are reserved from principal registration, and generic document APIs defensively reject them, so they cannot bypass profile authorization.

The planning profile is closed in v1. Cross-record validation records the observed community generation and SQLite compares it inside the same `BEGIN IMMEDIATE` transaction before advancing the projection, preventing independent local cores from committing incompatible stale decisions. Projects, tasks, events, and dependencies have fixed meanings. All-day dates use exclusive ends; timed events use UTC epoch milliseconds plus an IANA-style zone; recurrence is bounded; dependency cycles fail closed. Calendar and Gantt APIs are separate views of the same records.

## Consequences

Independent calendar and Gantt APP principals can consume and update one semantic namespace without sharing tokens or database tables. SQL projections may evolve independently as long as they derive deterministically from the signed profile records.

Profile grants enforce local community scoping but are not replicated membership or group authorization. The submitting APP identifier is device-attested by the local core but is not a separate p2panda author key. Future `GroupPolicy` must authorize remote profile records. Full iCalendar recurrence, TZDB pinning, CalDAV, profile negotiation over transports, signed third-party profile publication, mappings, and profile-version migration are deferred.

Changing the canonical planning manifest requires a new profile version and digest. Existing signed bytes are never rewritten.
