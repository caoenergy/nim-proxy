/* ===== settings — config-driven, not metrics-driven: fetched from
   /api/config on tab entry and after every save (the server filters by role
   BEFORE serialization, so sections render purely from data presence). Only
   the lane-state chips ride a 5s in-place refresh. ===== */
let SET = null, setSub = 'access';

async function sPost(path, body) {
  const r = await fetch(path, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  let j = {}; try { j = await r.json(); } catch {}
  if (!r.ok || j.ok === false) throw new Error((j.error && j.error.message) || j.error || `Request failed (${r.status})`);
  return j;
}
/* inline feedback next to the control that caused it (textContent — never markup) */
const note = (id, msg, ok) => { const el = $(id); if (el) { el.textContent = msg || ''; el.classList.toggle('ok', !!ok); } };

async function loadSettings() {
  try {
    const j = await (await fetch('/api/config')).json();
    if (!j.nim_keys) throw 0;
    SET = j;
  } catch { $('setbody').innerHTML = '<div class="empty">Could not load settings — reload to sign in again</div>'; return; }
  renderSettings();
}

const TRASH = '<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M2.5 4h11M6.5 4V2.5h3V4M4 4l.8 9.5h6.4L12 4M6.5 7v4M9.5 7v4"/></svg>';
const LOCK = '<svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><rect x="3.5" y="7" width="9" height="6.5" rx="1.5"/><path d="M5.5 7V5a2.5 2.5 0 015 0v2"/></svg>';
const SICONS = {
  access: '<svg viewBox="0 0 16 16"><circle cx="11" cy="5" r="2.5"/><path d="M9.2 6.8L2.5 13.5M4.8 11.2l1.5 1.5"/></svg>',
  server: '<svg viewBox="0 0 16 16"><rect x="2" y="2.5" width="12" height="4.5" rx="1.5"/><rect x="2" y="9" width="12" height="4.5" rx="1.5"/><path d="M4.6 4.75h.01M4.6 11.25h.01"/></svg>',
  users: '<svg viewBox="0 0 16 16"><circle cx="6" cy="5.5" r="2.2"/><path d="M2 13.5c.5-2.4 2-3.6 4-3.6s3.5 1.2 4 3.6M10.4 3.6a2.2 2.2 0 010 3.8M11.8 10c1.3.5 2 1.6 2.3 3.5"/></svg>',
  account: '<svg viewBox="0 0 16 16"><circle cx="8" cy="5" r="2.5"/><path d="M3 14c.6-2.8 2.4-4.2 5-4.2s4.4 1.4 5 4.2"/></svg>',
};

function renderSettings() {
  const subs = [['access', 'Access & keys'], ['server', 'Server'], ['users', 'Users'], ['account', 'Account']]
    .filter(([id]) => id === 'access' || id === 'account' || (id === 'server' ? !!SET.server : !!SET.users));
  if (!subs.some(([id]) => id === setSub)) setSub = 'access';
  $('setnav').innerHTML = subs.map(([id, label]) =>
    `<button role="tab" data-sub="${id}" aria-selected="${id === setSub}"><span class="nbar"></span>${SICONS[id]}<span>${escapeHtml(label)}</span></button>`).join('');
  for (const b of $('setnav').querySelectorAll('button'))
    b.addEventListener('click', () => { setSub = b.dataset.sub; renderSettings(); });
  ({ access: renderAccess, server: renderServer, users: renderUsers, account: renderAccount })[setSub]();
}

/* live lane state for one key row (also patched in place by the 5s refresh) */
function keyState(k) {
  if (!k.enabled) return { text: 'disabled', cls: 'kstate dim' };
  if (k.cooldown_ms > 0) return { text: `cooldown ${Math.ceil(k.cooldown_ms / 1000)}s`, cls: 'kstate warnc' };
  if (k.in_window != null) return { text: `${+k.in_window} / ${+k.rpm} in window`, cls: 'kstate' };
  return { text: 'unassigned', cls: 'kstate dim' };
}
const poolNote = () => `${+SET.pool.enabled} enabled in pool · Total ${+SET.pool.capacity_rpm} rpm`;
const clampInt = (v, lo, hi, dflt) => { const n = Math.round(+v); return isFinite(n) ? Math.max(lo, Math.min(hi, n)) : dflt; };

async function copyText(text, btn) {
  const old = btn.textContent;
  try { await navigator.clipboard.writeText(text); btn.textContent = 'copied ✓'; }
  catch { btn.textContent = 'copy failed'; }
  setTimeout(() => { btn.textContent = old; }, 1500);
}

/* show-once client-key secret: modal only closes on an explicit Done, so a
   stray re-render can't eat the one chance to copy it */
function showSecret(name, secret) {
  $('modal-body').innerHTML =
    `<p data-style="margin:0 0 12px;color:var(--ink-2);font-size:13px">Client key <b>${escapeHtml(name)}</b> is ready. Copy it now — <b data-style="color:var(--amber-lt)">you won't see this again.</b></p>
    <div class="secretbox">${escapeHtml(secret)}</div>
    <div data-style="display:flex;gap:10px;justify-content:flex-end;margin-top:16px">
      <button class="pbtn" id="modal-copy">Copy key</button>
      <button class="gbtn" id="modal-done">Done</button>
    </div>`;
  $('modal-copy').addEventListener('click', () => copyText(secret, $('modal-copy')));
  $('modal-done').addEventListener('click', () => $('modal').classList.remove('show'));
  $('modal').classList.add('show');
}

function renderAccess() {
  const admin = !!SET.users;
  const ownerChip = o => admin ? ` <span class="tag">${escapeHtml(o)}</span>` : '';
  const keyRows = SET.nim_keys.map((k, i) => {
    const st = keyState(k);
    return `<div class="krow${k.enabled ? '' : ' koff'}">
      <div data-style="min-width:0">
        <div class="kmask">nvapi-••••${escapeHtml(k.last4)}${ownerChip(k.owner)}</div>
        <div class="kmeta">fp ${escapeHtml(String(k.fingerprint).slice(0, 8))} · ${k.lane != null ? `slot ${+k.lane + 1}` : k.enabled ? 'unassigned' : 'off'}</div>
      </div>
      <span class="${st.cls}" data-ksfp="${escapeHtml(k.fingerprint)}">${escapeHtml(st.text)}</span>
      <span class="rpmwrap"><input class="sin num" type="number" min="1" max="10000" value="${+k.rpm}" data-rpm="${i}"><span class="unitl">rpm</span></span>
      <button class="tog" role="switch" aria-checked="${!!k.enabled}" data-tog="${i}" title="${k.enabled ? 'Disable — its rate window stays warm' : 'Enable'}"></button>
      ${k.guarded
        ? `<span class="klock" title="The pool keeps at least one enabled superuser key — add or enable another key before removing this one">${LOCK}</span>`
        : `<button class="dbtn icon" data-kdel="${i}" title="Remove key">${TRASH}</button>`}
    </div>`;
  }).join('');
  const ckRows = SET.client_keys.map((ck, i) =>
    `<div class="krow">
      <span data-style="font-weight:600;flex:0 1 160px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="${escapeHtml(ck.name)}">${escapeHtml(ck.name)}</span>
      <span class="kmask" data-style="color:var(--ink-25);font-weight:500">npk_••••••••${escapeHtml(ck.last4)}</span>
      ${admin ? `<span class="tag">${escapeHtml(ck.owner)}</span>` : ''}
      <button class="dbtn" data-style="margin-left:auto" data-ckdel="${i}">Revoke</button>
    </div>`).join('');
  $('setbody').innerHTML = `
    <div class="card mb">
      <h2>${admin ? 'NIM keys' : 'My NIM keys'} <span class="note" id="pool-note">${escapeHtml(poolNote())}</span></h2>
      <p class="shint">Keys you add join the shared pool as yours${admin ? '' : ' — only you and admins see they exist'}. Changes apply live: kept keys keep their rate windows, disabled keys re-enable warm.</p>
      <div>${keyRows || '<div class="empty">No NIM keys yet — add one below</div>'}</div>
      <div class="addrow">
        <input id="nk-key" class="sin" data-style="flex:1;min-width:200px" type="password" placeholder="nvapi-… paste a new NIM key" autocomplete="off" spellcheck="false">
        <span class="rpmwrap"><input id="nk-rpm" class="sin num" type="number" min="1" max="10000" value="40"><span class="unitl">rpm</span></span>
        <button class="pbtn" id="nk-add">Validate &amp; add</button>
        <button class="gbtn" id="nk-force" hidden>Add anyway</button>
      </div>
      <div class="serr" id="nk-err"></div>
    </div>
    <div class="card mb">
      <h2>${admin ? 'Client API keys' : 'My API keys'} <span class="note">bearer tokens your clients use</span></h2>
      <div>${ckRows || '<div class="empty">No client keys yet</div>'}</div>
      <div class="addrow">
        <input id="ck-name" class="sin" data-style="flex:1;min-width:200px" placeholder="name this key, e.g. laptop-cli" maxlength="64" autocomplete="off" spellcheck="false">
        <button class="pbtn" id="ck-add">+ Generate key</button>
      </div>
      <div class="serr" id="ck-err"></div>
    </div>
    <div class="card">
      <h2>Connection</h2>
      <div class="congrid">
        <div class="conbox">
          <div class="slabel">Point your client's base URL at</div>
          <div data-style="display:flex;align-items:center;gap:10px"><span class="kmask" data-style="flex:1;overflow:hidden;text-overflow:ellipsis">${escapeHtml(location.origin + '/v1')}</span><button class="gbtn" id="copy-base">copy</button></div>
        </div>
        <div class="conbox">
          <div class="slabel">API access mode</div>
          <div class="kmask" data-style="color:${SET.mode === 'keyed' ? 'var(--green-lt)' : 'var(--amber-lt)'}">● ${SET.mode === 'keyed' ? 'API key required' : 'Open — no authentication'}</div>
          <div class="kmeta" data-style="margin-top:4px">${SET.mode === 'keyed' ? 'clients authenticate with a client API key' : 'anyone who can reach /v1 can use the pool'}</div>
        </div>
      </div>
    </div>`;
  const body = $('setbody');
  for (const el of body.querySelectorAll('[data-rpm]')) el.addEventListener('change', async () => {
    const k = SET.nim_keys[+el.dataset.rpm];
    try {
      await sPost('/api/settings/nim-keys', { set: { fingerprint: k.fingerprint, rpm: clampInt(el.value, 1, 10000, +k.rpm) } });
      await loadSettings();
    } catch (e) { note('nk-err', e.message); }
  });
  for (const el of body.querySelectorAll('[data-tog]')) el.addEventListener('click', async () => {
    const k = SET.nim_keys[+el.dataset.tog];
    try {
      await sPost('/api/settings/nim-keys', { set: { fingerprint: k.fingerprint, enabled: !k.enabled } });
      await loadSettings();
    } catch (e) { note('nk-err', e.message); }
  });
  for (const el of body.querySelectorAll('[data-kdel]')) el.addEventListener('click', async () => {
    const k = SET.nim_keys[+el.dataset.kdel];
    if (!confirm(`Remove key nvapi-••••${k.last4} from the pool? In-flight requests finish; its rate window is lost.`)) return;
    try { await sPost('/api/settings/nim-keys', { remove: k.fingerprint }); await loadSettings(); }
    catch (e) { note('nk-err', e.message); }
  });
  /* validate first; "Add anyway" appears only after a failed probe */
  const addKey = async () => {
    await sPost('/api/settings/nim-keys', { add: { key: $('nk-key').value.trim(), rpm: clampInt($('nk-rpm').value, 1, 10000, 40) } });
    await loadSettings();
  };
  $('nk-add').addEventListener('click', async () => {
    const key = $('nk-key').value.trim();
    if (!key) return note('nk-err', 'Paste a NIM key first.');
    $('nk-add').disabled = true; $('nk-add').textContent = 'Validating…';
    try {
      const v = await sPost('/api/settings/validate-key', { key });
      note('nk-err', `✓ ${Array.isArray(v.models) ? v.models.length : +v.models} models · Adding…`, true);
      await addKey();
      return;
    } catch (e) {
      note('nk-err', `Validation failed: ${e.message}`);
      $('nk-force').hidden = false;
    }
    $('nk-add').disabled = false; $('nk-add').textContent = 'Validate & add';
  });
  $('nk-force').addEventListener('click', async () => {
    if (!$('nk-key').value.trim()) return note('nk-err', 'Paste a NIM key first.');
    try { await addKey(); } catch (e) { note('nk-err', e.message); }
  });
  for (const el of body.querySelectorAll('[data-ckdel]')) el.addEventListener('click', async () => {
    const ck = SET.client_keys[+el.dataset.ckdel];
    if (!confirm(`Revoke "${ck.name}"? Clients using it stop working immediately.`)) return;
    try { await sPost('/api/settings/clients', { remove: ck.name }); await loadSettings(); }
    catch (e) { note('ck-err', e.message); }
  });
  $('ck-add').addEventListener('click', async () => {
    const name = $('ck-name').value.trim();
    if (!name) return note('ck-err', 'Name the key first — e.g. laptop-cli.');
    try {
      const j = await sPost('/api/settings/clients', { add: { name } });
      showSecret(name, j.secret);
      await loadSettings();
    } catch (e) { note('ck-err', e.message); }
  });
  $('copy-base').addEventListener('click', () => copyText(location.origin + '/v1', $('copy-base')));
}

function renderServer() {
  const sv = SET.server, L = sv.limits;
  const num = (id, label, val, unit, step) => `<div><div class="slabel">${label}</div>
    <span class="rpmwrap" data-style="display:flex"><input id="${id}" class="sin" data-style="width:100%;text-align:right" type="number" min="0"${step ? ` step="${step}"` : ''} value="${+val}">${unit ? `<span class="unitl">${unit}</span>` : ''}</span></div>`;
  const ovr = Object.entries(sv.governor.overrides || {});
  const retainedFrom = sv.history.available_from == null
    ? 'No data yet'
    : at(STAMP, +sv.history.available_from * 1000);
  const historyBytes = NUM_GROUPED.format(+sv.history.file_bytes || 0);
  $('setbody').innerHTML = `
    <div class="card mb">
      <h2>API access mode
        <span class="pills" data-style="margin-left:auto">
          <button data-mode="open" aria-pressed="${SET.mode === 'open'}">Open (no authentication)</button>
          <button data-mode="keyed" aria-pressed="${SET.mode === 'keyed'}">API key required</button>
        </span></h2>
      <p class="shint">Controls <b>/v1</b> client calls only. <b>API key required</b> needs a bearer token; <b>Open</b> accepts anyone who can reach /v1 — trusted networks only.</p>
      ${SET.mode === 'keyed' && !SET.client_keys.length ? '<p class="shint" data-style="color:var(--amber-lt)">Requiring an API key with no client keys locks every client out — generate one under Access &amp; keys.</p>' : ''}
      <div class="serr" id="mode-err"></div>
    </div>
    <div class="card mb">
      <h2>Upstream &amp; limits <button class="pbtn" id="save-limits" data-style="margin-left:auto">Save</button></h2>
      <div class="slabel">Upstream base URL · saving clears the model-catalog cache</div>
      <input id="sv-base" class="sin" data-style="width:100%" value="${escapeHtml(sv.base_url)}" spellcheck="false">
      <div class="limgrid">
        ${num('sv-maxwait', 'Max wait', L.max_wait_secs, 's')}
        ${num('sv-heartbeat', 'Heartbeat', L.heartbeat_secs, 's')}
        ${num('sv-idle', 'Stream idle', L.stream_idle_secs, 's')}
        ${num('sv-timeout', 'Request timeout', L.request_timeout_secs, 's')}
        ${num('sv-ttl', 'Models TTL', L.models_ttl_secs, 's')}
        ${num('sv-inflight', 'Max in-flight', L.max_inflight, '')}
      </div>
      <div data-style="display:flex;align-items:center;gap:10px;margin-top:16px">
        <button class="tog" role="switch" id="sv-strict" aria-checked="${!!L.strict_passthrough}"></button>
        <span data-style="font-size:12.5px">Strict passthrough <span class="kmeta" data-style="display:inline">— reject params the upstream doesn't accept</span></span>
      </div>
      <div class="serr" id="limits-err"></div>
    </div>
    <div class="card mb">
      <h2>Model limits
        <span data-style="margin-left:auto;display:inline-flex;align-items:center;gap:8px"><span class="kmeta">adaptive</span>
        <button class="tog" role="switch" id="gov-tog" aria-checked="${!!sv.governor.enabled}"></button></span></h2>
      <p class="shint">Absorbs NIM worker-exhaustion per model without cooling down keys. Each model self-tunes: engages on the first exhaustion, climbs while stable, dissolves after a long clean stretch. Set an override only if you know a model's ceiling.</p>
      <div>${ovr.map(([m, cap], i) =>
        `<span class="ochip"><span title="${escapeHtml(m)}">${escapeHtml(m)}</span><b>${+cap} max</b><button data-govdel="${i}" title="Remove override">×</button></span>`).join('')
        || '<span class="kmeta">No overrides set</span>'}</div>
      <div class="addrow">
        <input id="gov-model" class="sin" data-style="flex:1;min-width:200px" placeholder="model id, e.g. moonshotai/kimi-k2.5" autocomplete="off" spellcheck="false">
        <span class="rpmwrap"><input id="gov-cap" class="sin num" type="number" min="1" max="10000" value="8"><span class="unitl">max</span></span>
        <button class="gbtn" id="gov-add">+ override</button>
      </div>
      <div class="serr" id="gov-err"></div>
    </div>
    <div class="card">
      <h2>History &amp; dashboard <button class="pbtn" id="save-history" data-style="margin-left:auto">Save</button></h2>
      <div class="limgrid" data-style="margin-top:6px">
        <div><div class="slabel">Default time range</div>
          <span class="rpmwrap" data-style="display:flex"><input id="sv-default-days" class="sin" data-style="width:100%;text-align:right" type="number" min="1" step="1" value="${+sv.dashboard.default_window_days}"><span class="unitl">days</span></span></div>
        <div><div class="slabel">History retention · 0 = unlimited</div>
          <span class="rpmwrap" data-style="display:flex"><input id="sv-retention-days" class="sin" data-style="width:100%;text-align:right" type="number" min="0" step="1" value="${+sv.history.days}"><span class="unitl">days</span></span></div>
        <div><div class="slabel">Availability SLO</div>
          <span class="rpmwrap" data-style="display:flex"><input id="sv-slo" class="sin" data-style="width:100%;text-align:right" type="number" min="0.1" max="100" step="0.1" value="${+sv.dashboard.slo_target_percent}"><span class="unitl">%</span></span></div>
      </div>
      <div class="congrid" data-style="margin-top:16px">
        <div class="conbox"><div class="slabel">Oldest data point</div><div class="kmeta" data-style="display:block">${escapeHtml(retainedFrom)}</div></div>
        <div class="conbox"><div class="slabel">Data file</div><div class="kmeta" data-style="display:block">${escapeHtml(historyBytes)} bytes</div></div>
      </div>
      ${sv.history.compaction_pending ? '<p class="shint" data-style="color:var(--amber-lt)">compaction pending</p>' : ''}
      <div class="serr" id="history-err"></div>
    </div>`;
  for (const b of $('setbody').querySelectorAll('[data-mode]')) b.addEventListener('click', async () => {
    if (b.dataset.mode === SET.mode) return;
    try { await sPost('/api/settings/clients', { mode: b.dataset.mode }); await loadSettings(); }
    catch (e) { note('mode-err', e.message); }
  });
  $('sv-strict').addEventListener('click', () =>
    $('sv-strict').setAttribute('aria-checked', $('sv-strict').getAttribute('aria-checked') !== 'true'));
  $('save-limits').addEventListener('click', async () => {
    const base = $('sv-base').value.trim();
    const nums = {
      max_wait_secs: 'sv-maxwait', heartbeat_secs: 'sv-heartbeat', models_ttl_secs: 'sv-ttl',
      stream_idle_secs: 'sv-idle', request_timeout_secs: 'sv-timeout', max_inflight: 'sv-inflight',
    };
    const limits = { strict_passthrough: $('sv-strict').getAttribute('aria-checked') === 'true' };
    for (const [field, id] of Object.entries(nums)) {
      const n = Math.round(+$(id).value);
      if (!isFinite(n) || n < 0) return note('limits-err', 'Every limit needs a whole number ≥ 0.');
      limits[field] = n;
    }
    try {
      if (base !== sv.base_url) await sPost('/api/settings/upstream', { base_url: base });
      await sPost('/api/settings/limits', limits);
      await loadSettings();
      note('limits-err', 'Saved.', true);
    } catch (e) { note('limits-err', e.message); }
  });
  $('gov-tog').addEventListener('click', async () => {
    try { await sPost('/api/settings/governor', { enabled: !sv.governor.enabled }); await loadSettings(); }
    catch (e) { note('gov-err', e.message); }
  });
  for (const b of $('setbody').querySelectorAll('[data-govdel]')) b.addEventListener('click', async () => {
    try { await sPost('/api/settings/governor', { remove_override: ovr[+b.dataset.govdel][0] }); await loadSettings(); }
    catch (e) { note('gov-err', e.message); }
  });
  $('gov-add').addEventListener('click', async () => {
    const model = $('gov-model').value.trim(), cap = Math.round(+$('gov-cap').value);
    if (!model || !(cap >= 1)) return note('gov-err', 'Give a model id and a cap ≥ 1.');
    try { await sPost('/api/settings/governor', { set_override: { model, cap } }); await loadSettings(); }
    catch (e) { note('gov-err', e.message); }
  });
  $('save-history').addEventListener('click', async () => {
    const defaultDays = Math.round(+$('sv-default-days').value);
    const retention = Math.round(+$('sv-retention-days').value);
    const slo = +$('sv-slo').value;
    if (!isFinite(defaultDays) || defaultDays < 1)
      return note('history-err', 'Default time range must be at least 1 day.');
    if (!isFinite(retention) || retention < 0)
      return note('history-err', 'History retention must be 0 or more days.');
    if (!isFinite(slo) || slo <= 0 || slo > 100)
      return note('history-err', 'SLO target must be greater than 0 and at most 100.');
    try {
      await sPost('/api/settings/history', {
        days: retention,
        default_window_days: defaultDays,
        slo_target_percent: slo,
      });
      await loadSettings();
      note('history-err', 'Saved.', true);
    } catch (e) { note('history-err', e.message); }
  });
}

function renderUsers() {
  const RCLS = { superuser: 'superuser', admin: 'admin', user: 'user' };
  const rows = SET.users.map((u, i) => `<div class="krow">
    <span class="umono">${escapeHtml((u.username[0] || '?').toUpperCase())}</span>
    <div data-style="min-width:0">
      <div data-style="font-weight:600;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(u.username)}</div>
      <div class="kmeta">${+u.nim_keys} NIM · ${+u.client_keys} client</div>
    </div>
    <span class="rbadge ${RCLS[u.role] || 'user'}" data-style="margin-left:auto">${escapeHtml(String(u.role).toUpperCase())}</span>
    ${u.role !== 'superuser' ? `<button class="gbtn" data-urp="${i}">Reset password</button>
      <select class="sin" data-urole="${i}" title="Role">
        <option value="admin"${u.role === 'admin' ? ' selected' : ''}>admin</option>
        <option value="user"${u.role === 'user' ? ' selected' : ''}>user</option>
      </select>
      <button class="dbtn" data-udel="${i}">Delete</button>`
      : `<span class="note" title="The superuser changes its own password in Account settings">self-managed</span>`}
  </div>`).join('');
  $('setbody').innerHTML = `<div class="card">
    <h2>Users <span class="note">deleting a user pulls their NIM keys from the pool &amp; stops their clients</span></h2>
    <div>${rows}</div>
    <div class="addrow">
      <input id="u-name" class="sin" data-style="flex:1;min-width:150px" placeholder="new username" autocomplete="off" spellcheck="false">
      <input id="u-pass" class="sin" data-style="flex:1;min-width:150px" type="password" placeholder="initial password (10+ chars)" autocomplete="new-password">
      <span class="pills"><button data-urolepick="user" aria-pressed="true">user</button><button data-urolepick="admin" aria-pressed="false">admin</button></span>
      <button class="pbtn" id="u-add">Add user</button>
    </div>
    <div class="serr" id="u-err"></div>
  </div>`;
  const body = $('setbody');
  for (const b of body.querySelectorAll('[data-urp]')) b.addEventListener('click', async () => {
    const u = SET.users[+b.dataset.urp];
    const pw = prompt(`New password for ${u.username} (at least 10 characters):`);
    if (pw == null) return;
    if (pw.length < 10) return note('u-err', 'Password must be at least 10 characters.');
    try {
      await sPost('/api/settings/users', { reset_password: { username: u.username, new_password: pw } });
      note('u-err', `Password reset for ${u.username}.`, true);
    } catch (e) { note('u-err', e.message); }
  });
  for (const s of body.querySelectorAll('[data-urole]')) s.addEventListener('change', async () => {
    const u = SET.users[+s.dataset.urole];
    try { await sPost('/api/settings/users', { set_role: { username: u.username, role: s.value } }); await loadSettings(); }
    catch (e) { note('u-err', e.message); }
  });
  for (const b of body.querySelectorAll('[data-udel]')) b.addEventListener('click', async () => {
    const u = SET.users[+b.dataset.udel];
    if (!confirm(`Delete ${u.username}? Their NIM keys leave the pool and their API keys stop working.`)) return;
    try { await sPost('/api/settings/users', { remove: u.username }); await loadSettings(); }
    catch (e) { note('u-err', e.message); }
  });
  let newRole = 'user';
  for (const b of body.querySelectorAll('[data-urolepick]')) b.addEventListener('click', () => {
    newRole = b.dataset.urolepick;
    for (const o of body.querySelectorAll('[data-urolepick]')) o.setAttribute('aria-pressed', o === b);
  });
  $('u-add').addEventListener('click', async () => {
    const username = $('u-name').value.trim(), password = $('u-pass').value;
    if (!username) return note('u-err', 'Pick a username.');
    if (password.length < 10) return note('u-err', 'Initial password must be at least 10 characters.');
    try { await sPost('/api/settings/users', { add: { username, password, role: newRole } }); await loadSettings(); }
    catch (e) { note('u-err', e.message); }
  });
}

function renderAccount() {
  $('setbody').innerHTML = `<div class="card" data-style="max-width:560px">
    <h2>Account</h2>
    <div class="slabel">Username</div>
    <input class="sin" data-style="width:100%" value="${escapeHtml(SET.username)}" disabled>
    <div class="slabel" data-style="margin-top:14px">Current password</div>
    <input id="a-cur" class="sin" data-style="width:100%" type="password" autocomplete="current-password">
    <div class="slabel" data-style="margin-top:14px">New password · at least 10 characters</div>
    <input id="a-new" class="sin" data-style="width:100%" type="password" autocomplete="new-password">
    <div class="slabel" data-style="margin-top:14px">Confirm new password</div>
    <input id="a-conf" class="sin" data-style="width:100%" type="password" autocomplete="new-password">
    <p class="shint" data-style="margin-top:14px">Changing your password signs out your other sessions.</p>
    <button class="pbtn" id="a-save">Update password</button>
    <div class="serr" id="a-err"></div>
  </div>`;
  $('a-save').addEventListener('click', async () => {
    const nw = $('a-new').value;
    if (nw.length < 10) return note('a-err', 'New password must be at least 10 characters.');
    if (nw !== $('a-conf').value) return note('a-err', 'Passwords do not match.');
    $('a-save').disabled = true; // password hashing is deliberately slow
    try {
      await sPost('/api/settings/account', { current_password: $('a-cur').value, new_password: nw });
      for (const id of ['a-cur', 'a-new', 'a-conf']) $(id).value = '';
      note('a-err', 'Password updated — this session was refreshed.', true);
    } catch (e) { note('a-err', e.message); }
    $('a-save').disabled = false;
  });
}

/* keep the lane-state chips honest while the page sits on Access & keys —
   patch text in place so inputs and focus survive */
setInterval(async () => {
  if (activeTab !== 'settings' || setSub !== 'access' || !SET) return;
  try {
    const j = await (await fetch('/api/config')).json();
    if (!j.nim_keys) return;
    SET.pool = j.pool;
    const byFp = new Map(j.nim_keys.map(k => [k.fingerprint, k]));
    for (const el of document.querySelectorAll('[data-ksfp]')) {
      const k = byFp.get(el.dataset.ksfp);
      if (!k) continue;
      const st = keyState(k);
      el.textContent = st.text;
      el.className = st.cls;
    }
    const pn = $('pool-note');
    if (pn) pn.textContent = poolNote();
  } catch {}
}, 5000);
