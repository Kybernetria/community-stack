#!/usr/bin/env bash
set -euo pipefail

fail_if_found() {
  local pattern=$1
  shift
  if rg -n "$pattern" "$@"; then
    echo "architecture boundary violation: /$pattern/ found in $*" >&2
    exit 1
  fi
}

# Domain vocabulary is data-only and has no infrastructure dependencies.
fail_if_found 'crate::|rusqlite|loro::|p2panda|tokio|chacha20|blake3|std::fs|std::net|reqwest|hyper|axum' src/domain

# Ports are owned by the application and may mention only domain vocabulary.
fail_if_found 'crate::adapters|rusqlite|loro::|p2panda|chacha20' src/ports.rs

# Use cases depend on ports, never concrete adapters or third-party engines.
fail_if_found 'crate::adapters|rusqlite|loro::|p2panda|chacha20|blake3|std::net|reqwest|hyper|axum' src/application.rs src/application

# Concrete adapters implement inward-facing ports and never invoke siblings.
fail_if_found 'crate::adapters::|super::(loro|p2panda|sqlite|local_api)' src/adapters

echo "architecture boundaries: ok"
