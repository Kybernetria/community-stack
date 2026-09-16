'use strict';
const $ = id => document.getElementById(id);
const session = location.hash.slice(1);
if (session) history.replaceState(null, '', location.pathname);
const pendingKey = 'community-hub.pending.v1';
let community = $('community').value, view = 'dashboard', documents = [], listCursor = null;
let changes = [], changeCursor = 0, current = null, pending = null, busy = false, epoch = 0, dirty = false;
function status(message, state = 'info') { $('status').textContent = message; $('status').dataset.state = state; }
function el(tag, value, cls) { const node = document.createElement(tag); if (value !== undefined) node.textContent = value; if (cls) node.className = cls; return node; }
function button(label, action, cls = 'secondary') { const node = el('button', label, cls); node.type = 'button'; node.addEventListener('click', () => guard(action)); return node; }
async function guard(action) { try { await action(); } catch (e) { status(e.message, e.code === 'PROTOCOL_ERROR' ? 'offline' : 'error'); } }
async function api(method, params = {}) {
  if (!session) throw new Error('Session missing. Open the complete one-time URL printed by the gateway.');
  let response;
  try { response = await fetch('/api', {method: 'POST', headers: {'Content-Type': 'application/json', 'X-Community-Hub-Session': session}, body: JSON.stringify({method, params})}); }
  catch (_) { const e = new Error('Gateway is offline. Start it and open its printed URL. Your pending command is kept for an exact retry.'); e.code = 'PROTOCOL_ERROR'; throw e; }
  const payload = await response.json();
  if (!payload.ok) { const e = new Error(payload.error.message); Object.assign(e, payload.error); throw e; }
  return payload.result;
}
function snapshot() { return {revision: current.revision, state: structuredClone(current.state)}; }
function pendingUI() { $('pending').hidden = !pending; $('retry').disabled = busy; $('discard').disabled = busy; if (pending) $('pending-description').textContent = `${pending.method} · ${pending.params.community_id} · ${pending.params.document_id || pending.params.record_id}. Retry reuses the exact saved command.`; }
function clearPending() { localStorage.removeItem(pendingKey); pending = null; pendingUI(); }
async function write(method, params) {
  if (busy) return;
  if (pending) throw new Error('Resolve the pending command before starting another write.');
  const command = {method, params: {...params, community_id: community, command_id: crypto.randomUUID()}};
  // Fail closed when durable browser storage is unavailable or full.
  localStorage.setItem(pendingKey, JSON.stringify(command));
  pending = command; pendingUI();
  await retry();
}
async function retry() {
  if (!pending || busy) return;
  busy = true; pendingUI(); status('Saving to your local core…');
  const command = structuredClone(pending);
  try {
    const doc = await api(command.method, command.params);
    clearPending();
    if (command.method.startsWith('planning.')) await loadPlanning();
    if (!command.method.startsWith('planning.') && doc.community_id === community) {
      dirty = false; current = doc; renderEditor();
      try { await refreshData(); } catch (e) { status(`Saved durably, but the overview could not refresh: ${e.message}`, 'error'); return; }
    }
    status('Saved durably. Your community is up to date.', 'success');
  } catch (e) {
    if (e.code === 'REVISION_CONFLICT') {
      // Never rebase a stale write automatically. Preserve the submitted draft separately.
      clearPending();
      let latest = e.latest;
      if (!latest) { try { latest = await api('document.get', {community_id: command.params.community_id, document_id: command.params.document_id}); } catch (_) {} }
      if (command.params.community_id === community) {
        dirty = false; current = latest || null; renderEditor();
        const details = el('details'); details.append(el('summary', 'Your unsaved draft — copy anything you want to keep'));
        details.append(el('pre', JSON.stringify(command.params, null, 2), 'draft')); $('editor').prepend(details);
        showView(command.method === 'note.save' ? 'notes' : 'tables', false);
      }
      status(latest ? e.message : `${e.message} Reload failed; refresh before editing.`, 'conflict');
    } else { status(e.message + ' The exact command is saved for retry.', e.code === 'PROTOCOL_ERROR' ? 'offline' : 'error'); }
  } finally { busy = false; pendingUI(); }
}
function canLeave() { return !dirty || confirm('Leave this unsaved draft?'); }
function showView(next, check = true) {
  if (busy && check) return;
  if (check && next !== view && !canLeave()) return;
  view = next;
  if (current && ((next === 'notes' && current.state.meta?.kind === 'table') || (next === 'tables' && current.state.meta?.kind !== 'table'))) { current = null; dirty = false; renderEditor(); }
  for (const node of document.querySelectorAll('nav button')) { if (node.dataset.view === next) node.setAttribute('aria-current', 'page'); else node.removeAttribute('aria-current'); }
  $('planning').hidden = next !== 'planning';
  if (next === 'planning') guard(loadPlanning);
  $('dashboard').hidden = next !== 'dashboard'; $('library').hidden = !['notes', 'tables'].includes(next); $('activity').hidden = next !== 'activity';
  $('page-title').textContent = {dashboard: 'A little more connected.', notes: 'Notes & wiki', tables: 'A place for every detail.', activity: 'Your community, in motion.', planning: 'Good things on the horizon.'}[next];
  $('new-document').textContent = next === 'tables' ? '＋ New table' : '＋ New note';
  $('library-description').textContent = next === 'tables' ? 'Simple tables for the things you organize together.' : 'A home for your community’s knowledge.';
  renderDocuments();
}
async function loadDocuments(reset = true) {
  const generation = epoch, selected = community;
  const page = await api('document.list', {community_id: selected, after: reset ? null : listCursor});
  // document.list has no titles. Hydrate each bounded page with document.get.
  const hydrated = [];
  for (let i = 0; i < page.documents.length; i += 6) {
    const batch = await Promise.all(page.documents.slice(i, i + 6).map(d => api('document.get', {community_id: selected, document_id: d.document_id})));
    hydrated.push(...batch);
  }
  if (generation !== epoch) return;
  documents = reset ? hydrated : [...documents, ...hydrated]; listCursor = page.next_cursor;
  $('count').textContent = `${documents.length}${listCursor !== null ? '+' : ''}`;
  $('count-detail').textContent = listCursor !== null ? 'Loaded documents · load more for the full count' : 'Notes, tables, and shared knowledge';
  renderDocuments();
}
function renderDocuments() {
  const list = $('documents'); list.replaceChildren();
  const query = $('search').value.toLocaleLowerCase();
  const items = documents.filter(d => (view !== 'tables' ? d.state.meta?.kind !== 'table' : d.state.meta?.kind === 'table') && `${d.document_id} ${d.state.meta?.title || ''}`.toLocaleLowerCase().includes(query));
  for (const doc of items) {
    const node = button('', async () => { if (busy || !canLeave()) return; const gen = epoch; status('Opening document…'); const result = await api('document.get', {community_id: community, document_id: doc.document_id}); if (gen !== epoch) return; current = result; dirty = false; renderEditor(); renderDocuments(); status('Document ready.'); });
    node.append(el('strong', doc.state.meta?.title || doc.document_id), el('small', `${doc.document_id} · rev ${doc.revision}`)); node.setAttribute('aria-pressed', String(current?.document_id === doc.document_id)); list.append(node);
  }
  if (!items.length) list.append(el('p', query ? 'No matching documents in the loaded pages.' : 'Nothing here yet. Start with a new document.', 'empty'));
  $('more-documents').hidden = listCursor === null;
}
function field(form, label, id, value, multiline = false) {
  const caption = el('label', label); caption.htmlFor = id; const input = el(multiline ? 'textarea' : 'input'); input.id = id; input.value = value; if (multiline) input.rows = id === 'note-body' ? 13 : 4;
  input.addEventListener('input', () => { dirty = true; }); form.append(caption, input); return input;
}
function renderEditor() {
  const target = $('editor'); target.replaceChildren();
  if (!current) { target.append(el('p', 'Choose a document to get started.', 'empty')); return; }
  const head = el('div', undefined, 'editor-heading'); head.append(el('h2', current.state.meta?.title || current.document_id), el('span', `Revision ${current.revision}`, 'badge')); target.append(head, el('small', current.document_id));
  if (current.state.meta?.kind === 'table') { renderTable(target); return; }
  if (current.state.meta?.kind && current.state.meta.kind !== 'note') { target.append(el('p', 'This document uses another app’s format and cannot be edited as a note.')); return; }
  const form = el('form');
  const title = field(form, 'Title', 'note-title', current.state.meta?.title || ''); title.required = true; title.maxLength = 240;
  const body = field(form, 'Your note', 'note-body', current.state.body || '', true); body.maxLength = 100000;
  const metadata = Object.fromEntries(Object.entries(current.state.meta || {}).filter(([key]) => !['title', 'kind'].includes(key)));
  const meta = field(form, 'Metadata · JSON object with simple values', 'note-meta', JSON.stringify(metadata, null, 2), true);
  const actions = el('div', undefined, 'editor-actions'); const save = el('button', 'Save note'); save.type = 'submit'; actions.append(save); form.append(actions);
  form.addEventListener('submit', e => { e.preventDefault(); guard(async () => {
    const value = JSON.parse(meta.value);
    if (!value || Array.isArray(value) || typeof value !== 'object' || Object.values(value).some(v => v !== null && typeof v === 'object')) throw new Error('Metadata must be a JSON object with simple values.');
    await write('note.save', {document_id: current.document_id, snapshot: snapshot(), title: title.value, body: body.value, metadata: value});
  }); }); target.append(form);
}
function renderTable(target) {
  const columns = Object.keys(current.state.columns || {}).sort();
  const actions = el('div', undefined, 'editor-actions');
  actions.append(button('＋ Add row', () => rowForm(target, columns), ''), button('Export CSV', async () => {
    const result = await api('table.csv', {community_id: community, document_id: current.document_id});
    const url = URL.createObjectURL(new Blob([result.csv], {type: 'text/csv;charset=utf-8'})); const a = el('a'); a.href = url; a.download = 'community-table.csv'; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000); status('CSV exported.', 'success');
  })); target.append(actions);
  const wrap = el('div', undefined, 'table-wrap'); const table = el('table'); const caption = el('caption', current.state.meta.title, 'sr-only'); table.append(caption);
  const head = el('thead'), hr = el('tr'); for (const col of [...columns, null]) { const th = el('th', col === null ? 'Actions' : current.state.columns[col]); th.scope = 'col'; hr.append(th); } head.append(hr); table.append(head);
  const tbody = el('tbody');
  for (const row of Object.keys(current.state.rows || {}).sort()) {
    const tr = el('tr'); for (const col of columns) tr.append(el('td', String(current.state.cells?.[JSON.stringify([row, col])] ?? '')));
    const td = el('td'); td.append(button('Edit', () => rowForm(target, columns, row)), button('Delete', async () => { if (confirm(`Delete row ${row}?`)) await write('row.delete', {document_id: current.document_id, snapshot: snapshot(), row_id: row}); })); tr.append(td); tbody.append(tr);
  }
  table.append(tbody); wrap.append(table); target.append(wrap);
  if (!tbody.children.length) target.append(el('p', 'No rows yet. Add your first one.', 'empty'));
}
function rowForm(target, columns, row = '') {
  if (!canLeave()) return;
  $('row-form')?.remove(); const form = el('form'); form.id = 'row-form'; form.append(el('h2', row ? 'Edit row' : 'Add a row'));
  const id = field(form, 'Row ID', 'row-id', row || crypto.randomUUID()); id.required = true; id.maxLength = 64; id.readOnly = !!row;
  const fields = columns.map((col, index) => {
    const old = current.state.cells?.[JSON.stringify([row, col])];
    const input = field(form, current.state.columns[col], `cell-${index}`, old === undefined ? '' : String(old));
    return {col, input, old};
  });
  const save = el('button', 'Save row'); save.type = 'submit'; const actions = el('div', undefined, 'editor-actions'); actions.append(save, button('Cancel', () => { dirty = false; form.remove(); })); form.append(actions);
  form.addEventListener('submit', e => { e.preventDefault(); guard(() => write('row.put', {document_id: current.document_id, snapshot: snapshot(), row_id: id.value, values: Object.fromEntries(fields.map(({col, input, old}) => [col, old !== undefined && String(old) === input.value ? old : input.value]))})); });
  target.append(form); id.focus();
}
function activityItem(change) {
  const node = el('article', undefined, 'activity-item'); node.append(el('span', '↗', 'activity-icon')); const content = el('div'); content.append(el('strong', change.document_id));
  content.append(el('p', `${change.operation || 'Document change'} · ${change.revision !== undefined ? `revision ${change.revision}` : 'revision unavailable'}`));
  content.append(el('p', `Operation ${change.operation_hash}`)); content.append(el('p', change.timestamp || (change.timestamp_ms ? new Date(change.timestamp_ms).toLocaleString() : 'Timestamp unavailable'))); node.append(content); return node;
}
async function loadActivity(reset = true) {
  const gen = epoch;
  const result = await api('document.changes', {community_id: community, after: reset ? 0 : changeCursor});
  if (gen !== epoch) return;
  changes = reset ? result.changes : [...changes, ...result.changes]; changeCursor = result.next_cursor;
  $('feed').replaceChildren(...changes.map(activityItem)); if (!changes.length) $('feed').append(el('p', 'No activity yet. Create a note or table to begin.', 'empty'));
  $('more-activity').hidden = !result.has_more;
  $('recent').replaceChildren(...changes.slice(-3).reverse().map(activityItem)); $('recent').classList.toggle('empty', !changes.length);
  if (!changes.length) $('recent').textContent = 'A fresh start. Your first change will appear here.';
  else if (result.has_more) $('recent').prepend(el('p', 'Recent within loaded history. Load more activity to reach the latest changes.', 'muted'));
}
async function refreshData() {
  const results = await Promise.allSettled([loadDocuments(), loadActivity()]);
  const failed = results.find(r => r.status === 'rejected'); if (failed) throw failed.reason;
}
async function refresh() {
  status('Refreshing your workspace…'); $('refresh').disabled = true;
  try {
    const health = await api('health'); $('health').textContent = health.status || (health.ready === true ? 'Ready' : 'Connected'); $('readiness').textContent = `${health.operations ?? '—'} operations · integrity ${health.sqlite_quick_check || 'not reported'}`;
    await refreshData(); status(health.status === 'degraded' ? 'Core is reachable but reports degraded health. Check the core before continuing.' : 'Connected to your local workspace.', health.status === 'degraded' ? 'error' : 'success');
  } catch (e) { $('health').textContent = 'Unavailable'; $('readiness').textContent = 'Check your core and APP capability'; status(e.message, e.code === 'PROTOCOL_ERROR' ? 'offline' : 'error'); }
  finally { $('refresh').disabled = false; }
}
function createDialog() {
  if (busy || !canLeave()) return;
  $('create-title').textContent = view === 'tables' ? 'Create a table' : 'Create a note'; $('column-field').hidden = view !== 'tables'; $('new-columns').required = view === 'tables'; $('new-id').value = `${view === 'tables' ? 'tables' : 'notes'}/${crypto.randomUUID()}`; $('new-title').value = ''; $('create-dialog').showModal(); $('new-title').focus();
}
for (const node of document.querySelectorAll('[data-view]')) node.addEventListener('click', () => showView(node.dataset.view));
$('search').addEventListener('input', renderDocuments);
$('refresh').addEventListener('click', refresh);
$('refresh-activity').addEventListener('click', () => guard(() => loadActivity()));
$('more-activity').addEventListener('click', () => guard(() => loadActivity(false)));
$('more-documents').addEventListener('click', () => guard(() => loadDocuments(false)));
$('retry').addEventListener('click', () => guard(retry));
$('discard').addEventListener('click', () => guard(() => { if (confirm('Discard the pending retry? A disconnected write may already have committed; refresh to check before creating another.')) { clearPending(); status('Pending retry discarded. Refresh to check the stored state.'); } }));
$('new-document').addEventListener('click', createDialog);
$('hero-note').addEventListener('click', () => { showView('notes'); createDialog(); });
$('close-create').addEventListener('click', () => $('create-dialog').close());
$('create-form').addEventListener('submit', e => { e.preventDefault(); guard(async () => {
  const id = $('new-id').value.trim(), title = $('new-title').value.trim();
  if (!id || !title) throw new Error('Enter a document ID and title.');
  if (view === 'tables') {
    const labels = $('new-columns').value.split('\n').map(s => s.trim()).filter(Boolean);
    if (!labels.length || labels.length > 32) throw new Error('Enter 1–32 column names.');
    $('create-dialog').close(); await write('table.create', {document_id: id, title, columns: Object.fromEntries(labels.map((label, i) => [`column-${String(i + 1).padStart(2, '0')}`, label]))});
  } else { $('create-dialog').close(); await write('note.save', {document_id: id, snapshot: {revision: 0, state: {}}, title, body: '', metadata: {}}); }
}); });
$('community-form').addEventListener('submit', e => { e.preventDefault(); if (busy || !canLeave()) return; community = $('community').value.trim(); if (!community) return; epoch++; dirty = false; current = null; documents = []; changes = []; listCursor = null; changeCursor = 0; $('selected-community').textContent = community; $('community-label').textContent = `${community.toUpperCase()} / WORKSPACE`; renderEditor(); renderDocuments(); refresh(); });
async function loadPlanning() {
  const gen = epoch;
  $('planning-status').textContent = 'Checking planning access…';
  try {
    const [calendar, gantt] = await Promise.all([api('planning.calendar.list', {community_id: community}), api('planning.gantt.get', {community_id: community})]);
    if (gen !== epoch) return;
    $('planning-status').textContent = 'Planning connected. Calendar and project views are current.';
    const list = $('planning-records'); list.replaceChildren(); list.classList.remove('empty');
    for (const [label, records] of [['Projects', gantt.projects], ['Tasks', gantt.tasks], ['Calendar', calendar.events]]) {
      list.append(el('h2', label));
      if (!records.length) list.append(el('p', `No ${label.toLowerCase()} yet.`, 'muted'));
      for (const record of records) {
        const item = el('article', undefined, 'activity-item'); const text = el('div'); text.append(el('strong', record.title));
        text.append(el('p', `${record.project_id || record.task_id || record.event_id} · ${record.status}`));
        if (record.task_id) text.append(el('p', `Task ${record.task_id} · ${record.progress_percent}% complete`));
        if (record.timing) text.append(el('p', record.timing.kind === 'all_day' ? `${record.timing.start_date} → ${record.timing.end_date_exclusive} (exclusive)` : new Date(record.timing.start_at_ms).toLocaleString()));
        item.append(text); list.append(item);
      }
    }
  } catch (e) {
    if (gen !== epoch) return;
    $('planning-records').replaceChildren(el('p', 'Planning data is unavailable.', 'empty'));
    $('planning-status').textContent = ['FORBIDDEN', 'UNAUTHENTICATED', 'UNKNOWN_METHOD', 'METHOD_NOT_FOUND'].includes(e.code) ? 'Planning needs setup: ask the local administrator to grant this APP read/write access to community.planning@1 for this community. See the README. Notes and tables remain available.' : e.message;
  }
}
$('refresh-planning').addEventListener('click', () => guard(loadPlanning));
$('plan-kind').addEventListener('change', () => {
  const kind = $('plan-kind').value;
  $('plan-project-field').hidden = kind === 'project'; $('plan-task-field').hidden = kind !== 'task'; $('plan-event-field').hidden = kind !== 'event';
  $('plan-start').required = $('plan-end').required = kind === 'event';
});
$('planning-form').addEventListener('submit', e => { e.preventDefault(); guard(async () => {
  const kind = $('plan-kind').value;
  const params = {record_id: crypto.randomUUID(), title: $('plan-title').value, description: $('plan-description').value, status: $('plan-status').value};
  if (kind !== 'project') params.project_id = $('plan-project').value.trim();
  if (kind === 'task') params.progress_percent = Number($('plan-progress').value);
  if (kind === 'event') { params.start_date = $('plan-start').value; params.end_date_exclusive = $('plan-end').value; if (params.end_date_exclusive <= params.start_date) throw new Error('End date must follow the start date.'); }
  await write(`planning.${kind}.put`, params);
}); });
window.addEventListener('beforeunload', e => { if (dirty) { e.preventDefault(); e.returnValue = ''; } });
try { pending = JSON.parse(localStorage.getItem(pendingKey) || 'null'); pendingUI(); } catch (_) { status('Browser storage is unavailable. Writes require local storage for durable retries.', 'error'); }
refresh();
