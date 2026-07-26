# ADR 0001: Use low-level p2panda-core and p2panda-sync

- Status: accepted
- Date: 2026-07-26

## Decision

Use `p2panda-core` for signed records and low-level `p2panda-sync` ports for replication. Do not use high-level `p2panda::Node` as the authoritative core.

## Why

The high-level builder couples iroh network identity to operation signing and constructs a SQLx-backed store. This stack requires independent user/device/author/Reticulum/iroh/Loro identities and one rusqlite-owned transaction spanning accepted record, CRDT update, projections, log head, idempotency, and outbox.

## Consequences

We own adapters for p2panda repository/sync traits and more orchestration code. In return, transports remain replaceable and cannot become authorization or storage authorities. Pre-1.0 APIs are isolated behind local modules and exact versions/lock checksums.
