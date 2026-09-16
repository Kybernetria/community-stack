# Validation record

Implementation checked against repository base `7165239`.

| Check | Result in implementation environment |
| --- | --- |
| `scripts/check-boundaries.sh` | Passed |
| `python -m unittest discover -s tests/python -v` | Passed: 18 tests (7 existing, 11 gateway tests) |
| `python -m py_compile $(find clients examples rns-bridge -name '*.py' -type f -print)` | Passed |
| `node --check examples/community-hub/app.js` | Passed |
| `git diff --check` | Passed |
| `cargo fmt --all -- --check` | Blocked: `cargo: command not found` |
| `cargo clippy --locked --all-targets -- -D warnings` | Blocked: `cargo: command not found` |
| `cargo test --locked --all-targets` | Blocked: `cargo: command not found` |
| Live-core end-to-end validation | Not run: Rust binary cannot be built here |
| Browser visual / interaction testing | Blocked: Chromium absent; Playwright Chromium download timed out |

Existing Rust sources and tests were not modified. Existing CI retains all Rust
and Python checks and now additionally checks Community Hub JavaScript syntax.
The HTTP gateway tests bind an ephemeral loopback port and validate behavior with
an injected core client; one test exercises an actual unavailable Unix socket.
These tests do not substitute for live-core or visual browser testing. Run those
checks with the pinned Rust toolchain and a browser before treating this as ready
for regular use.
