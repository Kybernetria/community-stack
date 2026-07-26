# ADR 0002: One rusqlite-owned SQLite authority

- Status: accepted
- Date: 2026-07-26

## Decision

While the service is running, one dedicated thread owns the repository SQLite connection and all durable writes. Offline `init` and principal registration commands require the service to be stopped. No independent SQLx p2panda database is permitted.

## Why

A local write is acknowledged only if canonical operation bytes, Loro update, author-log head, projections, outbox, and idempotency response commit atomically. Two stores cannot provide that invariant during crashes or power loss.

## Consequences

Implement p2panda storage traits over this schema. Keep transactions short, use WAL/foreign keys/busy timeout/`synchronous=FULL`, use `BEGIN IMMEDIATE` for head updates, and perform backups via SQLite's online backup API. Projections remain disposable; accepted signed records/checkpoints are authoritative.
