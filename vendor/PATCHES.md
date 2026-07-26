# Vendor patches

## p2panda-store 0.7.0

Source: crates.io artifact for p2panda workspace tag `v0.7.0` / commit `37be61875f285956c906002d74a2d0370df5b2c1`.

Patch: change the package default feature from `["sqlite", "macros"]` to `["macros"]`.

Reason: `p2panda-sync` enables `p2panda-store` defaults. Its SQLx SQLite backend depends on a different `libsqlite3-sys` and would create the second transactional authority this architecture explicitly forbids. Trait modules remain enabled; this project will implement them over its one rusqlite-owned schema.

Upgrade gate: diff the complete vendored directory against the exact upstream crate and reapply/review this one feature change.
