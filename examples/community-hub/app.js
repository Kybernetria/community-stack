'use strict';
const $ = id => document.getElementById(id);
const session = location.hash.slice(1);
if (session) history.replaceState(null, '', location.pathname);
const pendingKey = 'community-hub.pending.v1';
let community = $('community').value, view = 'dashboard', documents = [], listCursor = null;
let changes = [], changeCursor = 0, current = null, pending = null, conflictDraft = null, busy = false, epoch = 0, dirty = false;
const MAX_HTTP_BODY = 256 * 1024;
const readSequence = {documents: 0, activity: 0, planning: 0, document: 0, refresh: 0};
const planningStatuses = new Set(['planned', 'active', 'completed', 'cancelled', 'archived']);
function status(message, state = 'info') { $('status').textContent = message; $('status').dataset.state = state; }
function utf8Bytes(value) { return new TextEncoder().encode(value).length; }
function validateText(value, label, maximum, {empty = false, controls = true} = {}) {
  if (typeof value !== 'string' || (!empty && !value.trim()) || utf8Bytes(value) > maximum || value.includes('\x00') || (controls && /[\u0000-\u001f\u007f-\u009f]/u.test(value))) throw new Error(`${label} must be meaningful and at most ${maximum} UTF-8 bytes.`);
  return value;
}
function validateId(value, label, maximum = 256) { return validateText(value, label, maximum); }
function validatePrimitive(value, label) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') return;
  if (typeof value === 'number' && Number.isFinite(value)) {
    if (!Number.isInteger(value) || Number.isSafeInteger(value)) return;
  }
  throw new Error(`${label} must be a finite JSON primitive.`);
}
function validateMetadata(metadata) {
  if (!metadata || Array.isArray(metadata) || typeof metadata !== 'object' || Object.keys(metadata).length > 32) throw new Error('Metadata must be an object with at most 32 fields.');
  for (const [key, value] of Object.entries(metadata)) {
    validateText(key, 'Metadata keys', 64);
    if (key === 'kind' || key === 'title') throw new Error('Title and kind have dedicated fields.');
    validatePrimitive(value, `Metadata value for ${key}`);
  }
}
function validateDate(value, label) {
  validateText(value, label, 10);
  if (!/^\d{4}-\d{2}-\d{2}$/u.test(value)) throw new Error(`${label} must be a valid date.`);
  const parsed = new Date(`${value}T00:00:00Z`);
  if (Number.isNaN(parsed.valueOf()) || parsed.toISOString().slice(0, 10) !== value) throw new Error(`${label} must be a valid date.`);
}
function validateWrite(method, params) {
  if (!params || typeof params !== 'object' || Array.isArray(params)) throw new Error('Write parameters must be an object.');
  validateId(params.community_id, 'Community ID');
  if (method.startsWith('planning.')) {
    const kind = method.split('.')[1];
    if (!['project', 'task', 'event'].includes(kind)) throw new Error('Unsupported planning write.');
    validateId(params.record_id, 'Planning record ID'); validateText(params.title, 'Planning title', 240);
    validateText(params.description, 'Planning description', 4000, {empty: true});
    if (!planningStatuses.has(params.status)) throw new Error('Choose a valid planning status.');
    if (kind !== 'project') { if (typeof params.project_id !== 'string') throw new Error('Project ID must be text.'); if (params.project_id.trim()) validateId(params.project_id, 'Project ID'); }
    if (kind === 'task' && (!Number.isInteger(params.progress_percent) || params.progress_percent < 0 || params.progress_percent > 100)) throw new Error('Progress must be a whole number from 0 to 100.');
    if (kind === 'event') { validateDate(params.start_date, 'Start date'); validateDate(params.end_date_exclusive, 'End date'); if (params.end_date_exclusive <= params.start_date) throw new Error('End date must follow the start date.'); }
    return;
  }
  validateId(params.document_id, 'Document ID');
  if (method === 'table.create') {
    validateText(params.title, 'Table title', 240);
    if (!params.columns || Array.isArray(params.columns) || typeof params.columns !== 'object' || Object.keys(params.columns).length < 1 || Object.keys(params.columns).length > 32) throw new Error('Use 1–32 column names.');
    for (const [key, label] of Object.entries(params.columns)) { validateId(key, 'Column ID', 64); validateText(label, 'Column name', 120); }
  } else {
    if (!params.snapshot || typeof params.snapshot !== 'object' || Array.isArray(params.snapshot) || !Number.isSafeInteger(params.snapshot.revision) || params.snapshot.revision < 0 || !params.snapshot.state || typeof params.snapshot.state !== 'object' || Array.isArray(params.snapshot.state)) throw new Error('The document snapshot is invalid. Refresh and try again.');
    if (method === 'note.save') {
      validateText(params.title, 'Note title', 240); if (typeof params.body !== 'string' || utf8Bytes(params.body) > 100000 || params.body.includes('\x00')) throw new Error('Note body must be at most 100000 UTF-8 bytes.');
      validateMetadata(params.metadata);
    } else {
      validateId(params.row_id, 'Row ID', 64);
      if (method === 'row.put') { if (!params.values || Array.isArray(params.values) || typeof params.values !== 'object') throw new Error('Row values must be an object.'); for (const value of Object.values(params.values)) validatePrimitive(value, 'Row value'); }
    }
  }
}
function validateSerializedCommand(command) {
  const serialized = JSON.stringify(command);
  if (typeof serialized !== 'string' || utf8Bytes(serialized) > MAX_HTTP_BODY) throw new Error('This write is too large for the local gateway.');
}
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
  readSequence.document++;
  const command = {method, params: {...params, community_id: community, command_id: crypto.randomUUID()}};
  // Validate before touching storage so rejected input cannot replace a durable v1 command.
  validateWrite(method, command.params);
  validateSerializedCommand(command);
  // Fail closed when durable browser storage is unavailable or full.
  const serialized = JSON.stringify(command);
  localStorage.setItem(pendingKey, serialized);
  pending = command; pendingUI();
  await retry();
}
function isDocumentWrite(method) { return ['note.save', 'table.create', 'row.put', 'row.delete'].includes(method); }
function writeView(method) { return method === 'note.save' ? 'notes' : ['table.create', 'row.put', 'row.delete'].includes(method) ? 'tables' : null; }
function renderConflictDraft() {
  const panel = $('conflict-draft');
  panel.hidden = !conflictDraft;
  if (!conflictDraft) return;
  const {command} = conflictDraft;
  $('conflict-description').textContent = `Your unsaved ${command.method} draft for ${command.params.community_id}. It remains here until you dismiss it.`;
  $('conflict-content').textContent = JSON.stringify(command.params, null, 2);
}
function showConflictDraft(command, latest) {
  conflictDraft = {command: structuredClone(command), latest};
  renderConflictDraft();
  if (command.params.community_id !== community || !isDocumentWrite(command.method)) return;
  dirty = false; current = latest || null;
  const latestView = latest?.state?.meta?.kind === 'table' ? 'tables' : writeView(command.method);
  showView(latestView, false);
}
async function retry() {
  if (!pending || busy) return;
  busy = true; readSequence.document++; pendingUI(); status('Saving to your local core…');
  // Do not validate or rebuild a v1 record here: retry sends its stored params verbatim.
  const command = structuredClone(pending);
  try {
    const result = await api(command.method, command.params);
    clearPending();
    const targetCommunity = command.params.community_id;
    if (command.method.startsWith('planning.')) {
      if (targetCommunity === community && view === 'planning') await loadPlanning();
    } else if (result.community_id === community) {
      dirty = false; current = result; renderEditor();
      try { await refreshData(); } catch (e) { status(`Saved durably, but the overview could not refresh: ${e.message}`, 'error'); return; }
    }
    if (targetCommunity === community) status('Saved durably. Your community is up to date.', 'success');
    else status(`Saved durably in ${targetCommunity}. Switch communities to view it.`, 'success');
  } catch (e) {
    if (e.code === 'REVISION_CONFLICT') {
      // Never rebase a stale write automatically. Preserve the submitted draft separately.
      clearPending();
      let latest = e.latest;
      if (!latest && isDocumentWrite(command.method)) { try { latest = await api('document.get', {community_id: command.params.community_id, document_id: command.params.document_id}); } catch (_) {} }
      showConflictDraft(command, latest);
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
  const sequence = ++readSequence.documents, generation = epoch, selected = community;
  const cursor = reset ? null : listCursor;
  const page = await api('document.list', {community_id: selected, after: cursor});
  // document.list has no titles. Hydrate each bounded page with document.get.
  const hydrated = [];
  for (let i = 0; i < page.documents.length; i += 6) {
    const batch = await Promise.all(page.documents.slice(i, i + 6).map(d => api('document.get', {community_id: selected, document_id: d.document_id})));
    hydrated.push(...batch);
  }
  if (generation !== epoch || sequence !== readSequence.documents) return;
  if (reset) documents = hydrated;
  else {
    const known = new Set(documents.map(d => d.document_id));
    documents = [...documents, ...hydrated.filter(d => !known.has(d.document_id))];
  }
  listCursor = page.next_cursor;
  $('count').textContent = `${documents.length}${listCursor !== null ? '+' : ''}`;
  $('count-detail').textContent = listCursor !== null ? 'Loaded documents · load more for the full count' : 'Notes, tables, and shared knowledge';
  renderDocuments();
}
function renderDocuments() {
  const list = $('documents'); list.replaceChildren();
  const query = $('search').value.toLocaleLowerCase();
  const items = documents.filter(d => (view !== 'tables' ? d.state.meta?.kind !== 'table' : d.state.meta?.kind === 'table') && `${d.document_id} ${d.state.meta?.title || ''}`.toLocaleLowerCase().includes(query));
  for (const doc of items) {
    const node = button('', async () => { if (busy || !canLeave()) return; const sequence = ++readSequence.document, gen = epoch, selected = community; status('Opening document…'); const result = await api('document.get', {community_id: selected, document_id: doc.document_id}); if (gen !== epoch || sequence !== readSequence.document || selected !== community) return; current = result; dirty = false; renderEditor(); renderDocuments(); status('Document ready.'); });
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
  const sequence = ++readSequence.activity, gen = epoch, selected = community;
  const cursor = reset ? 0 : changeCursor;
  const result = await api('document.changes', {community_id: selected, after: cursor});
  if (gen !== epoch || sequence !== readSequence.activity || selected !== community) return;
  if (reset) changes = result.changes;
  else {
    const known = new Set(changes.map(change => JSON.stringify(change)));
    changes = [...changes, ...result.changes.filter(change => !known.has(JSON.stringify(change)))];
  }
  changeCursor = result.next_cursor;
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
  const sequence = ++readSequence.refresh;
  status('Refreshing your workspace…'); $('refresh').disabled = true;
  try {
    const health = await api('health');
    if (sequence !== readSequence.refresh) return;
    $('health').textContent = health.status || (health.ready === true ? 'Ready' : 'Connected'); $('readiness').textContent = `${health.operations ?? '—'} operations · integrity ${health.sqlite_quick_check || 'not reported'}`;
    await refreshData();
    if (sequence !== readSequence.refresh) return;
    status(health.status === 'degraded' ? 'Core is reachable but reports degraded health. Check the core before continuing.' : 'Connected to your local workspace.', health.status === 'degraded' ? 'error' : 'success');
  } catch (e) { if (sequence !== readSequence.refresh) return; $('health').textContent = 'Unavailable'; $('readiness').textContent = 'Check your core and APP capability'; status(e.message, e.code === 'PROTOCOL_ERROR' ? 'offline' : 'error'); }
  finally { if (sequence === readSequence.refresh) $('refresh').disabled = false; }
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
$('dismiss-conflict').addEventListener('click', () => { conflictDraft = null; renderConflictDraft(); status('Conflict draft dismissed.'); });
$('new-document').addEventListener('click', createDialog);
$('hero-note').addEventListener('click', () => { showView('notes'); createDialog(); });
$('close-create').addEventListener('click', () => $('create-dialog').close());
$('create-form').addEventListener('submit', e => { e.preventDefault(); guard(async () => {
  const id = $('new-id').value.trim(), title = $('new-title').value.trim();
  if (view === 'tables') {
    const labels = $('new-columns').value.split('\n').map(s => s.trim()).filter(Boolean);
    const params = {document_id: id, title, columns: Object.fromEntries(labels.map((label, i) => [`column-${String(i + 1).padStart(2, '0')}`, label]))};
    validateWrite('table.create', {...params, community_id: community, command_id: 'validation'}); validateSerializedCommand({method: 'table.create', params: {...params, community_id: community, command_id: 'validation'}});
    await write('table.create', params); $('create-dialog').close();
  } else {
    const params = {document_id: id, snapshot: {revision: 0, state: {}}, title, body: '', metadata: {}};
    validateWrite('note.save', {...params, community_id: community, command_id: 'validation'}); validateSerializedCommand({method: 'note.save', params: {...params, community_id: community, command_id: 'validation'}});
    await write('note.save', params); $('create-dialog').close();
  }
}); });
$('community-form').addEventListener('submit', e => { e.preventDefault(); guard(async () => { if (busy || !canLeave()) return; const next = $('community').value.trim(); validateId(next, 'Community ID'); if (next === community) return; community = next; epoch++; dirty = false; current = null; documents = []; changes = []; listCursor = null; changeCursor = 0; $('selected-community').textContent = community; $('community-label').textContent = `${community.toUpperCase()} / WORKSPACE`; renderEditor(); renderDocuments(); await refresh(); }); });
async function loadPlanning() {
  const sequence = ++readSequence.planning, gen = epoch, selected = community;
  $('planning-status').textContent = 'Checking planning access…';
  try {
    const [calendar, gantt] = await Promise.all([api('planning.calendar.list', {community_id: selected}), api('planning.gantt.get', {community_id: selected})]);
    if (gen !== epoch || sequence !== readSequence.planning || selected !== community) return;
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
    if (gen !== epoch || sequence !== readSequence.planning || selected !== community) return;
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
try { pending = JSON.parse(localStorage.getItem(pendingKey) || 'null'); pendingUI(); renderConflictDraft(); } catch (_) { status('Browser storage is unavailable. Writes require local storage for durable retries.', 'error'); }
refresh();
