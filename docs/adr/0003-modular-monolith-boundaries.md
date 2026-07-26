# ADR 0003: Modular monolith with ports and adapters

- Status: accepted
- Date: 2026-07-26

## Decision

Ship one process and one database, but keep five directional boundaries:

```text
domain ← ports ← application ← adapters ← main composition root
```

- `domain/`: data-only business vocabulary grouped by documents, facts, replication, and security.
- `ports.rs`: interfaces owned by use cases (`Repository`, `DocumentEngine`, `ContentHasher`, `SecureLog`).
- `application.rs`: orchestration and transaction policy; the only layer that coordinates domains.
- `adapters/`: independent Loro, p2panda, SQLite, and local-IPC implementations.
- `main.rs`: constructs adapters and injects them into the application.

A CI boundary check rejects infrastructure imports in domain, ports, and application modules.

## Why

Separate services would add failure modes and distributed transactions before they provide value. A flat module tree would let SQLite, Loro, p2panda, API, and transport concerns leak into every use case. A modular monolith keeps deployment minimal while preserving seams that can be tested, replaced, or later extracted.

## Rules

1. Domain modules contain no I/O or third-party engine types.
2. Application code imports ports and domain values only.
3. Adapters never call each other. Cross-adapter coordination belongs in the application layer. Cryptographic and collision-resistant hashing is provided through `ContentHasher`; use cases do not import a hashing implementation.
4. The SQLite adapter owns transactions but not business decisions.
5. The Loro adapter owns CRDT mechanics but not authorization or persistence.
6. The secure-log adapter owns canonical encoding/signing/encryption but not transport.
7. Local API and future transports translate framing only; they do not apply state.
8. Add a domain module only for cohesive behavior/vocabulary, not one per database table.
9. Split into separate crates only when module privacy and CI checks become insufficient.

## Consequences

Use cases are testable with in-memory port implementations, and concrete engines are replaceable without rewriting orchestration. There is some explicit translation between domain records and adapter-native values; that duplication is intentional anti-corruption, not boilerplate to bypass.
