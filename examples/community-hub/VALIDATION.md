# Validation record

Implementation checked against repository base `022fd9c`.

| Check | Result in implementation environment |
| --- | --- |
| `scripts/check-boundaries.sh` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo clippy --locked --all-targets -- -D warnings` | Passed |
| `cargo test --locked --all-targets` | Passed |
| `python -m unittest discover -s tests/python -v` | Passed: 21 tests |
| `python -m py_compile $(find clients examples rns-bridge -name '*.py' -type f -print)` | Passed |
| `node --check examples/community-hub/app.js` | Passed |
| `git diff --check` | Passed |
| `python3 scripts/smoke-workspace.py --binary target/debug/community-stack` | Passed: live workspace and recovery smoke |

The Python gateway tests use real loopback HTTP requests with a fake IPC client
and do not need live credentials or a real community. Browser visual/interaction
testing was not run in this session; use a browser with the pinned Rust checks
before treating visual details as final.
