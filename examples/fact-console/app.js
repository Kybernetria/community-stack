const sessionSecret = window.location.hash.slice(1);
if (sessionSecret) window.history.replaceState(null, '', window.location.pathname);

const health = document.querySelector('#health');
const assertForm = document.querySelector('#assert-form');
const queryForm = document.querySelector('#query-form');
const schemaForm = document.querySelector('#schema-form');
const writeResult = document.querySelector('#write-result');
const inspectionResult = document.querySelector('#inspection-result');
const historyResult = document.querySelector('#history-result');
const adminResult = document.querySelector('#admin-result');
const retryButton = document.querySelector('#retry');
const newWriteButton = document.querySelector('#new-write');
const factsBody = document.querySelector('#facts');
let lastWrite = null;

async function rpc(method, params = {}) {
  const response = await fetch('/rpc', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'X-Community-Console-Session': sessionSecret,
    },
    body: JSON.stringify({method, params}),
  });
  const body = await response.json();
  if (!response.ok || !body.ok) throw new Error(body.error || `HTTP ${response.status}`);
  return body.result;
}

function freshIds() {
  assertForm.elements.claim_id.value ||= `claim-${crypto.randomUUID()}`;
  assertForm.elements.idempotency_key.value = crypto.randomUUID();
}

function showJson(element, value) {
  element.textContent = JSON.stringify(value, null, 2);
}

async function checkHealth() {
  try {
    const result = await rpc('health');
    health.textContent = `${result.status} · ${result.operations} operations`;
    health.className = 'status ok';
  } catch (error) {
    health.textContent = error.message;
    health.className = 'status error';
  }
}

function factFromForm() {
  const fields = new FormData(assertForm);
  return {
    community_id: fields.get('community_id'),
    claim_id: fields.get('claim_id'),
    subject: fields.get('subject'),
    predicate: fields.get('predicate'),
    object: JSON.parse(fields.get('object')),
    source: fields.get('source') || null,
    source_document_id: fields.get('source_document_id') || null,
    confidence: Number(fields.get('confidence')),
    schema_id: fields.get('schema_id'),
    schema_version: Number(fields.get('schema_version')),
    lifecycle_status: fields.get('lifecycle_status'),
    idempotency_key: fields.get('idempotency_key'),
  };
}

async function submitFact(payload) {
  writeResult.textContent = 'Committing claim revision…';
  try {
    const result = await rpc('fact.assert', payload);
    showJson(writeResult, result);
    await Promise.all([checkHealth(), runQuery()]);
  } catch (error) {
    writeResult.textContent = `Error: ${error.message}`;
  }
}

assertForm.addEventListener('submit', async (event) => {
  event.preventDefault();
  try {
    const candidate = factFromForm();
    if (lastWrite && candidate.idempotency_key === lastWrite.idempotency_key
        && JSON.stringify(candidate) !== JSON.stringify(lastWrite)) {
      throw new Error('payload changed while reusing a committed key; click Start new write');
    }
    lastWrite = candidate;
    retryButton.disabled = false;
    await submitFact(lastWrite);
  } catch (error) {
    writeResult.textContent = `Invalid form: ${error.message}`;
  }
});

retryButton.addEventListener('click', () => lastWrite && submitFact(lastWrite));
newWriteButton.addEventListener('click', () => {
  assertForm.elements.idempotency_key.value = crypto.randomUUID();
  lastWrite = null;
  retryButton.disabled = true;
  writeResult.textContent = 'New idempotency key created; edits will become a new signed revision.';
});

async function runQuery() {
  const fields = new FormData(queryForm);
  const params = {community_id: fields.get('community_id'), limit: 100};
  if (fields.get('subject')) params.subject = fields.get('subject');
  if (fields.get('predicate')) params.predicate = fields.get('predicate');
  try {
    renderFacts(await rpc('fact.query', params));
  } catch (error) {
    factsBody.replaceChildren(rowWithMessage(`Error: ${error.message}`));
  }
}

queryForm.addEventListener('submit', (event) => {
  event.preventDefault();
  runQuery();
});

function rowWithMessage(message) {
  const row = document.createElement('tr');
  const cell = document.createElement('td');
  cell.colSpan = 6;
  cell.className = 'empty';
  cell.textContent = message;
  row.append(cell);
  return row;
}

function renderFacts(facts) {
  factsBody.replaceChildren();
  if (!facts.length) {
    factsBody.append(rowWithMessage('No matching active claims.'));
    return;
  }
  for (const fact of facts) {
    const row = document.createElement('tr');
    for (const value of [fact.claim_id, fact.subject, fact.predicate, JSON.stringify(fact.object)]) {
      const cell = document.createElement('td');
      cell.textContent = value;
      row.append(cell);
    }
    const operation = document.createElement('td');
    const code = document.createElement('code');
    code.textContent = `${fact.source_operation_hash.slice(0, 12)}…`;
    code.title = fact.source_operation_hash;
    operation.append(code);
    row.append(operation);

    const action = document.createElement('td');
    const button = document.createElement('button');
    button.textContent = 'Inspect + history';
    button.addEventListener('click', () => inspectFact(fact));
    action.append(button);
    row.append(action);
    factsBody.append(row);
  }
}

async function inspectFact(fact) {
  inspectionResult.textContent = 'Inspecting linked projections…';
  historyResult.textContent = 'Loading bounded history…';
  const params = {community_id: fact.community_id, claim_id: fact.claim_id};
  try {
    const [inspection, history] = await Promise.all([
      rpc('fact.inspect', params),
      rpc('fact.history', {...params, limit: 25}),
    ]);
    showJson(inspectionResult, inspection);
    showJson(historyResult, history);
  } catch (error) {
    inspectionResult.textContent = `Error: ${error.message}`;
    historyResult.textContent = `Error: ${error.message}`;
  }
}

schemaForm.addEventListener('submit', async (event) => {
  event.preventDefault();
  const fields = new FormData(schemaForm);
  try {
    const result = await rpc('schema.register', {
      app_id: fields.get('app_id'),
      schema_id: fields.get('schema_id'),
      schema_version: Number(fields.get('schema_version')),
      predicate: fields.get('predicate'),
      object_schema: JSON.parse(fields.get('object_schema')),
      multiple_active_claims: fields.get('multiple_active_claims') === 'on',
      provenance_policy: fields.get('provenance_policy'),
    });
    showJson(adminResult, result);
  } catch (error) {
    adminResult.textContent = `Error: ${error.message}`;
  }
});

document.querySelector('#schema-list').addEventListener('click', async () => {
  try {
    showJson(adminResult, await rpc('schema.list', {
      app_id: schemaForm.elements.app_id.value,
      limit: 100,
    }));
  } catch (error) {
    adminResult.textContent = `Error: ${error.message}`;
  }
});

document.querySelector('#doctor').addEventListener('click', async () => {
  try {
    const [doctor, projection] = await Promise.all([
      rpc('system.doctor'),
      rpc('projection.check'),
    ]);
    showJson(adminResult, {doctor, projection});
  } catch (error) {
    adminResult.textContent = `Error: ${error.message}`;
  }
});

freshIds();
checkHealth();
