# Dependency security checks

Rust application dependencies remain exact-version requirements with the reviewed
`Cargo.lock`; no dependency upgrade is part of this change. GitHub Actions are
referenced by immutable commit SHA and retain a release comment beside each
reference.

A separate scheduled/manual workflow runs `cargo-audit` through the pinned
`actions-rust-lang/audit` v1 action. It is intentionally not part of the normal
push/pull-request build: the RustSec advisory database and the audit tool are
network-fetched, and advisory availability is not a reproducible local input.
Developers can run the same check locally when `cargo-audit` is installed:

```sh
cargo audit --locked
```

`cargo-deny` was considered but is not enabled yet. Its license/source policy
would need an explicit review for the transitive p2panda and platform-specific
artifacts; adding a broad default policy would create noisy, non-actionable
failures. Revisit it when the supported distribution matrix and SBOM policy are
frozen. This keeps offline development and the locked build unchanged while
still providing a periodic vulnerability signal.
