# Community Hub

A dependency-free development workspace for Community Stack: notes/wiki, sparse tables, a paged activity feed, and optional shared planning. Python 3.10+ and a modern browser are required. Everything goes through the existing v1 Unix-socket API and Python Workspace helper. The gateway never opens SQLite or imports Rust internals.

## Start from a fresh checkout

Run these commands from the repository root on a supported Linux system. Install Rust through rustup; the repository's `rust-toolchain.toml` selects the pinned toolchain.

```sh
cargo build --locked
./target/debug/community-stack init --data-dir ./data
./target/debug/community-stack register --data-dir ./data --id org.example.community-hub --role app \
  --token-file ./data/community-hub.capability
```

`--token-file` is recommended: it creates a new mode `0600` file without printing
this bearer capability. The legacy registration form still prints the APP token
once for compatibility. Store capabilities securely. Run `init` and `register` only with the core stopped. Do not reinitialize existing data. The APP identity owns its document namespace: use the same APP token to reopen the same workspace.

Start the core in one terminal:

```sh
./target/debug/community-stack serve --data-dir ./data
```

In another terminal, enter the APP token without putting it in command history (Bash):

```bash
read -rsp 'APP token: ' COMMUNITY_APP_TOKEN; echo
export COMMUNITY_APP_TOKEN
python3 examples/community-hub/server.py --socket ./data/community.sock --port 8766
```

Open the **complete URL printed by the gateway**, including `#session-secret`, for example `http://127.0.0.1:8766/#…`. The browser immediately removes the fragment from the address bar and retains the session secret in memory. A reload needs the original printed URL again; a gateway restart generates a new secret. Like Fact Console, this is a per-process secret delivered through a one-time URL, not a server-side single-redemption token. Do not share the URL. The only accepted hostname is `127.0.0.1`.

The APP token stays in Python and never reaches browser code. `--token` is supported explicitly, but environment input avoids exposing the token in process arguments. ADMIN tokens are neither accepted nor read by this gateway. There is no external service, CDN, build step, package installation, or browser network dependency.

## Use the workspace

1. Enter a community ID in the sidebar and press the arrow. The dashboard shows health, loaded document count, and activity. With more document pages, the count has a `+`; load all pages for an exact count.
2. Choose **Notes & wiki → New note**, enter a stable document ID and title, and create it. Write the body and choose **Save note**. Metadata accepts a JSON object of simple values, such as `{"category":"gardening","published":true}`. `kind` and `title` have dedicated fields. The current revision appears above the editor.
3. Search loaded notes or tables by ID or title. The API's list summaries lack titles, so the gateway/UI retrieve documents through `document.get` to hydrate each page. **Load more documents** expands the searchable collection.
4. Choose **Tables → New table**, enter one column label per line, then add rows. Edit or delete rows with the adjacent actions. New/edited cells are text; existing primitive values survive unchanged edits. Stable row and column IDs identify cells. **Export CSV** calls the existing `table_csv()` helper, which sorts columns and rows deterministically. CSV is a safe, lossy export, not a backup: text headings and string cells that begin with `=`, `+`, `-`, or `@` (including after leading whitespace) receive a leading apostrophe at export time, while numeric values remain numeric. The authoritative `Document` is never changed; use device backup/recovery for backup.
5. **Activity** uses `document.changes`. Refresh starts at the beginning; **Load more activity** passes `next_cursor` back unchanged. The API currently returns oldest-first document IDs and operation hashes, but no per-change revision or timestamp. Those fields are explicitly marked unavailable. Dashboard activity is recent within loaded history and says when later pages remain. Cursors are never decoded or used to infer revisions.

### Saves, disconnects, and conflicts

Before a write, the browser saves its exact request, base snapshot, and a fresh UUID command ID in localStorage. Every retry rebuilds the same mutation and uses the same idempotency key; every IPC attempt gets a fresh request ID. Writes are blocked if browser storage fails. Only one unresolved command is allowed at a time. Use **Retry exact write** after restarting the core/gateway and reopening its printed URL at the same port. Browser storage is scoped to that origin; use the same port and browser profile to recover it.

Successful writes clear the pending command. If the revision changed, the gateway reloads the latest document and returns HTTP 409. The editor displays that revision and a copyable unsaved draft. Review and manually reapply changes; the app never silently overwrites or automatically rebases them. Failed conflict reloads leave editing unavailable until a fresh document read. Discarding an ambiguous pending command does not undo a write that may already have committed: refresh before creating another.

Pending payloads contain document content on this device, but no APP token. They remain until resolved or explicitly discarded. Protect the browser profile accordingly. In-memory drafts are not autosaved. Browsers may eventually evict localStorage; export/backup of authoritative core data remains a separate concern.

## Optional planning profile

The Planning panel uses `planning.project.put`, `planning.task.put`, `planning.event.put`, `planning.calendar.list`, and `planning.gantt.get`. It creates projects, tasks with progress, and all-day events (exclusive end date), and displays the calendar and project/task views. It does not edit existing planning records, support recurrence, or display a timeline chart. Views are bounded to 100 records per collection by the gateway. Authorization or API availability errors remain local to the panel; notes and tables still work. No placeholder successful data is generated.

To grant access, stop the core and register a separate administrator if you do not already have one:

```sh
./target/debug/community-stack register --data-dir ./data --id device-admin --role admin
```

Restart the core. Enter the ADMIN token into a separate shell environment, then issue the grant directly through the existing Python IPC client. Never give this token to the GUI gateway:

```bash
read -rsp 'ADMIN token: ' COMMUNITY_ADMIN_TOKEN; echo
export COMMUNITY_ADMIN_TOKEN
PYTHONPATH=clients/python python3 - <<'PY'
import os
import uuid
from community_stack_ipc import CoreClient
client = CoreClient('./data/community.sock', os.environ['COMMUNITY_ADMIN_TOKEN'])
result = client.call('profile.grant', {
    'app_id': 'org.example.community-hub',
    'profile_id': 'community.planning',
    'community_id': 'garden-club',
    'can_read': True,
    'can_write': True,
}, uuid.uuid4().hex)
print(result)
PY
unset COMMUNITY_ADMIN_TOKEN
```

Use your actual APP registration ID and selected community. Grants are community-scoped and do not grant replicated membership. Refresh Planning after setup. See `protocol/profiles/community-planning-v1.md` for the profile's precise semantics and limitations.

## Gateway boundaries

- HTTP binds only to `127.0.0.1`; no public bind option exists.
- Exact Host, Origin, and constant-time session-header checks guard POST requests. Static files are explicitly allowlisted; source files and directory listings are unavailable.
- `/api` accepts only explicit workspace actions and five planning methods. It is not a generic RPC proxy. Note/table actions build `document.mutate` calls with required revision preconditions through Workspace.
- Request bodies are bounded to 256 KiB, with JSON shape/value validation, finite primitive values, bounded fields, no transfer encoding, and safe structured JSON errors. Socket reads time out.
- Restrictive CSP, no-store caching, no-referrer, frame denial, and permissions headers apply. User content is rendered with text nodes, never injected as HTML.
- No access logging, token echo, document-content logging, ADMIN capability, SQLite access, or automatic conflict overwrite.
- This follows Fact Console's development security model. It is not a production server and must not be reverse-proxied or exposed to the network.

## Verification

Run from the repository root:

```sh
scripts/check-boundaries.sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python -m unittest discover -s tests/python -v
python -m py_compile $(find clients examples rns-bridge -name '*.py' -type f -print)
```

The gateway tests exercise real loopback HTTP requests with a fake IPC client: allowlisting, Host/Origin/session rejection, malformed and oversized bodies, safe errors, static-file restrictions, exact retries with fresh request IDs, input validation, and revision-conflict reloads. Existing Workspace/IPC tests remain intact. No test needs live credentials or writes to a real community.
