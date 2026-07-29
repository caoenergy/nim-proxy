#!/usr/bin/env node
/*
 * Render the embedded dashboard against captured API payloads, drive it the
 * way an operator does, and fail on any uncaught page exception.
 *
 * This exists because `cargo test` asserts on served HTML *text* and never
 * parses or executes the page JavaScript, and `node --check` proves only that
 * the syntax parses. Every page bug this project has shipped got past both.
 * The specific bug that motivated this file threw on chart hover, and because
 * the poll loop's catch treats any throw as "connection lost", a healthy proxy
 * rendered a red "Disconnected" badge and froze most of the tab.
 *
 * Stdlib only, matching scripts/formatter_fixture.js: no package.json, no npm
 * install, no Playwright. Node 22 ships a WebSocket client, so the Chrome
 * DevTools Protocol is reachable with nothing added to the repo.
 *
 * Usage:
 *   node scripts/render_check.js              # fail on page errors
 *   node scripts/render_check.js --locale en-XA   # also report untranslated runs
 *   CHROME=/path/to/chrome node scripts/render_check.js
 */

'use strict';

const fs = require('fs');
const os = require('os');
const path = require('path');
const { spawn, execFileSync } = require('child_process');

const ROOT = path.resolve(__dirname, '..');
const FIXTURES = path.join(ROOT, 'tests', 'fixtures', 'api');

const args = process.argv.slice(2);
// Which embedded page to drive. The dashboard renders from captured payloads;
// the wizard has no payloads and is driven by filling and clicking instead.
// Both were being proved by hand-built one-off harnesses, which is more work
// than one committed check and leaves nothing behind.
const pageArg = (() => {
  const i = args.indexOf('--page');
  return i >= 0 ? args[i + 1] : 'dashboard';
})();
if (!['dashboard', 'setup'].includes(pageArg)) {
  console.error(`unknown --page ${pageArg} (expected: dashboard, setup)`);
  process.exit(2);
}
const IS_SETUP = pageArg === 'setup';
const PAGE_REL = path.join('src', `${pageArg}.html`);
const PAGE = path.join(ROOT, PAGE_REL);
const CATALOG_PREFIX = IS_SETUP ? 'setup-' : '';
const localeArg = (() => {
  const i = args.indexOf('--locale');
  return i >= 0 ? args[i + 1] : null;
})();
// Catalog values are escaped once at load, so no render helper may escape its
// label argument again (knowledge/decisions/message-catalog-and-escaping.md).
// English labels contain no escapable character, so a second escape is
// invisible until a real locale ships. This makes it visible now.
const escapeProbe = args.includes('--escape-probe');
// Appended to every catalog value under --escape-probe. See the mutation below.
const PROBE_MARKER = 'Ampersand';
// The `"` is not decoration. An unescaped value interpolated into a quoted
// attribute inside an innerHTML string breaks OUT of the attribute, and the
// parser then reads the rest as further attribute names — so a `<b` or `"`
// shows up as an ATTRIBUTE NAME, which is unambiguous. Scanning attribute
// VALUES cannot work: getAttribute() decodes entities, so a correctly-escaped
// value and an unescaped one are byte-identical by the time the DOM has them.
const PROBE_SUFFIX = ' Ampersand & Quote\' <b>Tag</b> DQ"';
const PROBE_TAG_TEXT = 'Tag';

/* ---------- locate a browser ---------------------------------------------- */

function findChrome() {
  // CHROME is a hint, not a promise: if it does not exist, keep looking rather
  // than spawning a path that is not there.
  const candidates = [
    process.env.CHROME,
    '/opt/pw-browsers/chromium/chrome-linux/chrome',
    ...expandGlob('/opt/pw-browsers/chromium-*/chrome-linux/chrome'),
    ...expandGlob('/opt/pw-browsers/chromium_headless_shell-*/chrome-linux/headless_shell'),
    '/usr/bin/google-chrome-stable',
    '/usr/bin/google-chrome',
    '/usr/bin/chromium-browser',
    '/usr/bin/chromium',
  ];
  for (const c of candidates) if (c && fs.existsSync(c)) return c;
  return null;
}

function expandGlob(pattern) {
  const dir = path.dirname(path.dirname(path.dirname(pattern)));
  const rest = pattern.slice(dir.length + 1).split('/');
  if (!fs.existsSync(dir)) return [];
  return fs
    .readdirSync(dir)
    .filter((n) => new RegExp('^' + rest[0].replace(/\*/g, '.*') + '$').test(n))
    .map((n) => path.join(dir, n, ...rest.slice(1)));
}

/* ---------- minimal CDP client over Node's built-in WebSocket -------------- */

class CDP {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    this.listeners = [];
    ws.addEventListener('message', (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error ? reject(new Error(JSON.stringify(msg.error))) : resolve(msg.result);
      } else if (msg.method) {
        for (const fn of this.listeners) fn(msg);
      }
    });
  }

  static connect(url) {
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      ws.addEventListener('open', () => resolve(new CDP(ws)));
      ws.addEventListener('error', (e) => reject(new Error('ws: ' + e.message)));
    });
  }

  on(fn) {
    this.listeners.push(fn);
  }

  send(method, params = {}, sessionId) {
    const id = ++this.id;
    const payload = { id, method, params };
    if (sessionId) payload.sessionId = sessionId;
    this.ws.send(JSON.stringify(payload));
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }));
  }
}

let LINE_OFFSET = 0;

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function evaluateRaw(browser, sessionId, expression) {
  const r = await browser.send(
    'Runtime.evaluate',
    { expression, returnByValue: true, awaitPromise: true },
    sessionId,
  );
  if (r.exceptionDetails) throw new Error('evaluate threw: ' + r.exceptionDetails.text);
  return r.result.value;
}

/* ---------- build the page under test ------------------------------------- */

function buildPage(tmpdir) {
  let html = fs.readFileSync(PAGE, 'utf8');

  // The wizard fetches nothing at load; it only POSTs on user action. Requiring
  // the dashboard fixtures for it would be a fake dependency.
  const need = IS_SETUP ? [] : ['config.json', 'dashboard.json', 'dashboard-now.json'];
  for (const f of need) {
    const p = path.join(FIXTURES, f);
    if (!fs.existsSync(p)) {
      console.error(`missing fixture ${path.relative(ROOT, p)} — see ${path.relative(ROOT, path.join(FIXTURES, 'README.md'))}`);
      process.exit(2);
    }
  }
  const fixtures = Object.fromEntries(
    need.map((f) => [f, JSON.parse(fs.readFileSync(path.join(FIXTURES, f), 'utf8'))]),
  );

  if (localeArg) {
    const catalogFile = `${CATALOG_PREFIX}${localeArg}.json`;
    const cp = path.join(ROOT, 'locales', catalogFile);
    if (!fs.existsSync(cp)) {
      console.error(`missing locale ${path.relative(ROOT, cp)}`);
      process.exit(2);
    }
    const cat = fs.readFileSync(cp, 'utf8').trim();
    html = html.replace(
      /(<script type="application\/json" id="i18n-catalog">)[\s\S]*?(<\/script>)/,
      (_m, a, b) => a + cat + b,
    );
  }

  // Replay the captured payloads. Anything else (fonts, CDN logos) resolves to
  // a rejected promise, which is what an offline install already does.
  // In-page capture as well as CDP: the page boots from an async IIFE, so a
  // throw there surfaces as an unhandled rejection, which Runtime.exceptionThrown
  // does not reliably report. A gate blind to that is blind to boot failure.
  // The probe string carries both directions of the escape-once rule:
  //   `&` and `'`  — escaped twice, they surface as literal `&amp;` / `&#39;`
  //   `<b>`        — not escaped at all, it becomes a real ELEMENT in the DOM
  // Without the tag the probe was a double-escape detector only, structurally
  // blind to the missing-escape direction — which is the XSS direction.
  if (escapeProbe) {
    html = html.replace(
      /(<script type="application\/json" id="i18n-catalog">)([\s\S]*?)(<\/script>)/,
      (_m, a, json, b) => {
        const cat = JSON.parse(json);
        for (const k of Object.keys(cat.messages)) {
          const v = cat.messages[k];
          if (typeof v === 'string') cat.messages[k] = v + PROBE_SUFFIX;
          else v.en = v.en + PROBE_SUFFIX;
        }
        return a + JSON.stringify(cat) + b;
      },
    );
  }

  const stub = `<script>
window.__fetched = [];
window.__pageErrors = [];
window.addEventListener('error', function (e) {
  window.__pageErrors.push({ kind: 'error', msg: e.message,
    line: e.lineno, col: e.colno, stack: e.error && e.error.stack });
});
window.addEventListener('unhandledrejection', function (e) {
  var r = e.reason;
  window.__pageErrors.push({ kind: 'unhandledrejection',
    msg: String((r && r.message) || r), stack: r && r.stack });
});
(function () {
  const FIX = ${JSON.stringify(fixtures)};
  window.fetch = function (input) {
    const url = String(typeof input === 'string' ? input : input.url);
    window.__fetched.push(url);
    let body = null;
    if (url.includes('/api/config')) body = FIX['config.json'];
    else if (url.includes('/api/dashboard/now')) body = FIX['dashboard-now.json'];
    else if (url.includes('/api/dashboard')) body = FIX['dashboard.json'];
    // The wizard's only two endpoints. Hand-written rather than captured
    // because a real /setup claims a proxy and mints a secret; the shapes are
    // asserted against the Rust handlers by the openapi test.
    // Shapes taken from openapi.json (ValidateKeyResponse, SetupResponse /
    // MintedClientKey), not invented — a stub that answers in a shape the
    // server never sends would prove the page works against fiction. The
    // openapi test keeps that spec honest against the handlers.
    else if (url.includes('/setup/validate-key')) body = { ok: true, models: 63 };
    else if (url.endsWith('/setup')) {
      body = { ok: true, client_key: { name: 'default', secret: 'npk_probe_secret' } };
    }
    if (body === null) return Promise.reject(new Error('offline: ' + url));
    return Promise.resolve(new Response(JSON.stringify(body), {
      status: 200, headers: { 'Content-Type': 'application/json' },
    }));
  };
})();
</script>`;
  // Every injected line shifts the page's own line numbers, and a gate that
  // reports the wrong line is a gate nobody trusts. Record the shift and undo
  // it when reporting.
  LINE_OFFSET = (stub.match(/\n/g) || []).length;
  html = html.replace(/<head>/i, '<head>' + stub);

  const out = path.join(tmpdir, 'dashboard.html');
  fs.writeFileSync(out, html);
  return out;
}

/* ---------- the run -------------------------------------------------------- */


// A run of ASCII prose with no accented character, under a locale where every
// translated message is accented, is a string the catalog never reached.
const SCAN_UNTRANSLATED = `
  (() => {
    const out = [];
    const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let n = walk.nextNode(); n; n = walk.nextNode()) {
      const p = n.parentNode.nodeName;
      if (p === 'SCRIPT' || p === 'STYLE') continue;
      const t = n.textContent.trim();
      if (t.length < 3 || !/[a-zA-Z]{3}/.test(t)) continue;
      if (/[\\u00C0-\\u024F]/.test(t)) continue;   // accented: the catalog reached it
      out.push(t);
    }
    return Array.from(new Set(out));
  })()`;

// Values that originate in the API response, computed by the page's own
// helpers so this cannot drift from what it actually renders.
const dataLabelValues = (() => {
  // Data means values that arrived in the payload — nothing else. Scraping
  // rendered elements misclassifies our own generated names: "Slot 1" comes
  // from a template we wrote, not from the API.
  const out = new Set();
  for (const f of ['dashboard.json', 'dashboard-now.json']) {
    const p = path.join(FIXTURES, f);
    if (!fs.existsSync(p)) continue;
    const doc = JSON.parse(fs.readFileSync(p, 'utf8'));
    for (const bucket of ['totals', 'latest']) {
      for (const r of doc[bucket] || []) {
        for (const k of ['model', 'client']) {
          const v = (r.labels || {})[k];
          if (v && v !== 'none') out.add(v);
        }
      }
    }
  }
  return [...out];
})();

// Expand each payload value through the page's own helpers, so the exclusion
// set matches what it actually renders rather than what I assume it renders.
const SCAN_DATA_DERIVED = `
  (() => {
    const ids = ${JSON.stringify(dataLabelValues)};
    const out = new Set(ids);
    for (const id of ids) {
      try { out.add(prettyName(id)); } catch (e) {}
      try { out.add(publisher(id).name); } catch (e) {}
    }
    // Anything Intl produces is CLDR data, not catalog text — weekday names and
    // hour labels are correct-by-construction for the active locale and will
    // never be accented under en-XA. Ask the page's own cached formatters
    // rather than hardcoding a list that would drift from them.
    try { for (const d of DAYS) out.add(d); } catch (e) {}
    try {
      for (let h = 0; h < 24; h++) {
        out.add(HOUR_ONLY.format(Date.UTC(2024, 0, 1, h)));
        out.add(HOUR_ONLY.formatRange(Date.UTC(2024, 0, 1, h), Date.UTC(2024, 0, 1, h + 1)));
      }
    } catch (e) {}
    return [...out].filter(Boolean);
  })()`;

const DASH_TABS = ['overview', 'models', 'clients', 'reliability', 'capacity'];
/* The wizard has no tabs and no payloads: it is a form, so it has to be filled
   and clicked. Each step returns true once the panel it should have revealed is
   visible, so a step that silently does nothing fails loudly instead of letting
   the scan measure step 1 four times — the same mistake the dashboard's
   hash-vs-click bug caused. `errors` deliberately trips the validation paths,
   which is where eight of the wizard's messages live. */
const SETUP_STEPS = {
  errors: `(() => {
    $('username').value = 'op'; $('password').value = 'short';
    $('confirm').value = 'mismatch'; $('to2').click();
    const shown = ($('err').textContent || '').trim();
    return shown.length > 0 && !$('err').hidden;
  })()`,
  step1: `(() => {
    $('username').value = 'op'; $('password').value = 'probe-password-1';
    $('confirm').value = 'probe-password-1'; $('to2').click();
    return !$('step2').hidden;
  })()`,
  step2: `(async () => {
    $('newkey').value = 'nvapi-probe-key';
    $('addkey').click();
    for (let i = 0; i < 40 && $('to3').disabled; i++) await new Promise(r => setTimeout(r, 50));
    return !$('to3').disabled && /63/.test($('keylist').textContent || '');
  })()`,
  step3: `(() => {
    $('to3').click();
    const reviewed = ($('review').textContent || '').length > 0;
    // Toggle both ways: apiAccessLine() has two branches and they land in
    // different sinks — esc() into innerHTML on first render, textContent on
    // change. Leave it checked so step4 reaches showConnect rather than
    // navigating to /.
    $('mintkey').checked = false;
    $('mintkey').dispatchEvent(new Event('change'));
    $('mintkey').checked = true;
    $('mintkey').dispatchEvent(new Event('change'));
    return !$('step3').hidden && reviewed;
  })()`,
  step4: `(async () => {
    $('finish').click();
    for (let i = 0; i < 60 && $('step4').hidden; i++) await new Promise(r => setTimeout(r, 50));
    return !$('step4').hidden;
  })()`,
};
const TABS = IS_SETUP ? Object.keys(SETUP_STEPS) : DASH_TABS;

// The never-translate list has exactly one definition, in check_i18n.py. A JS
// copy would drift, and this list decides which "untranslated" runs are
// actually correct.
function frozenTokens() {
  const out = execFileSync('python3', ['-c',
    'import sys,json; sys.path.insert(0,"scripts"); import check_i18n; '
    + 'print(json.dumps(sorted(check_i18n.NEVER_TRANSLATE)))'],
    { cwd: ROOT, encoding: 'utf8' });
  return JSON.parse(out);
}

async function main() {
  const chrome = findChrome();
  if (!chrome) {
    console.error('no chromium found. Set CHROME=/path/to/chrome.');
    process.exit(2);
  }

  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'nimproxy-render-'));
  const pageFile = buildPage(tmpdir);

  const proc = spawn(chrome, [
    '--headless=new',
    '--disable-gpu',
    '--no-sandbox',
    '--hide-scrollbars',
    '--window-size=1440,2400',
    '--remote-debugging-port=0',
    `--user-data-dir=${path.join(tmpdir, 'profile')}`,
    'about:blank',
  ], { stdio: ['ignore', 'ignore', 'pipe'] });

  const wsUrl = await new Promise((resolve, reject) => {
    let buf = '';
    const timer = setTimeout(() => reject(new Error('browser did not report a debug port')), 20000);
    proc.stderr.on('data', (d) => {
      buf += d.toString();
      const m = buf.match(/ws:\/\/[^\s]+/);
      if (m) {
        clearTimeout(timer);
        resolve(m[0]);
      }
    });
    proc.on('exit', (c) => reject(new Error('browser exited early: ' + c)));
  });

  const browser = await CDP.connect(wsUrl);
  const { targetId } = await browser.send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await browser.send('Target.attachToTarget', { targetId, flatten: true });
  const S = sessionId;

  const errors = [];
  const consoleErrors = [];
  browser.on((msg) => {
    if (msg.sessionId !== S) return;
    if (msg.method === 'Runtime.exceptionThrown') {
      const d = msg.params.exceptionDetails;
      errors.push({
        text: (d.exception && d.exception.description) || d.text,
        line: d.lineNumber + 1,
        col: d.columnNumber,
      });
    }
    if (msg.method === 'Log.entryAdded' && msg.params.entry.level === 'error') {
      consoleErrors.push('[' + msg.params.entry.source + '] ' + msg.params.entry.text);
    }
    if (msg.method === 'Runtime.consoleAPICalled' && msg.params.type === 'error') {
      consoleErrors.push(msg.params.args.map((a) => a.value ?? a.description).join(' '));
    }
  });

  await browser.send('Runtime.enable', {}, S);
  await browser.send('Log.enable', {}, S);
  await browser.send('Page.enable', {}, S);
  await browser.send('DOM.enable', {}, S);
  await browser.send('Network.enable', {}, S);

  // The page render-blocks on the Google Fonts stylesheet. Offline it hangs
  // rather than failing, the parser never reaches the page's own <script>, and
  // a check that waits a fixed interval then measures the DOM concludes that a
  // page which never booted is fine. Block them so they fail immediately — the
  // same monogram/system-font fallback an offline install already takes.
  await browser.send('Network.setBlockedURLs', {
    urls: ['*fonts.googleapis.com*', '*fonts.gstatic.com*', '*unpkg.com*'],
  }, S);

  const loaded = new Promise((resolve) => {
    browser.on((m) => {
      if (m.sessionId === S && m.method === 'Page.loadEventFired') resolve();
    });
  });
  await browser.send('Page.navigate', { url: 'file://' + pageFile }, S);
  await Promise.race([loaded, sleep(20000)]);

  const state = await evaluateRaw(browser, S, 'document.readyState');
  if (state !== 'complete') {
    console.error(`FAIL — page never finished loading (readyState=${state}).`);
    proc.kill('SIGKILL');
    process.exit(1);
  }
  // let the first poll land and paint
  await sleep(1500);

  if (process.env.DEBUG) {
    const diag = await browser.send('Runtime.evaluate', {
      expression: `JSON.stringify({
        fetches: (window.__fetched || []).slice(0, 8),
        svgAny: document.querySelectorAll('svg').length,
        svgBig: Array.from(document.querySelectorAll('svg')).filter(s => {
          const r = s.getBoundingClientRect(); return r.width > 80 && r.height > 40; }).length,
        sections: Array.from(document.querySelectorAll('section[id], .tabpane, [data-tab]')).map(e => e.id).slice(0, 10),
        visibleH: document.body.getBoundingClientRect().height,
        traffic: !!document.getElementById('o-traffic'),
        trafficHTML: (document.getElementById('o-traffic') || {}).innerHTML ? 'populated' : 'EMPTY',
        hash: location.hash,
        stubRan: (typeof window.__pageErrors !== 'undefined'),
        readyState: document.readyState,
        booted: (typeof POLL_MS !== 'undefined'),
      })`,
      returnByValue: true,
    }, S);
    console.log('DEBUG ' + diag.result.value);
  }

  const evaluate = async (expression) => {
    const r = await browser.send(
      'Runtime.evaluate',
      { expression, returnByValue: true, awaitPromise: true },
      S,
    );
    if (r.exceptionDetails) {
      throw new Error('evaluate threw: ' + JSON.stringify(r.exceptionDetails.text));
    }
    return r.result.value;
  };

  /* Both runtimes must refuse to localize an attribute outside the allowlist.
     This was asserted by two knowledge pages and a lint comment while only ONE
     of the two pages actually enforced it, and the wizard shipped without the
     guard. Reasoning found that; nothing re-checks it, so assert it in-page on
     a synthetic element rather than trusting either description. */
  const attrGuard = await evaluate(`
    (() => {
      if (typeof applyStatic !== 'function') return 'applyStatic is not reachable';
      if (typeof I18N_ATTRS === 'undefined') return 'I18N_ATTRS is not defined';
      const host = document.createElement('div');
      // A localizable attribute must be set; a scripting one must be refused.
      host.innerHTML =
        '<span id="__probe_ok" data-i18n-attr="title:__missing_id"></span>' +
        '<span id="__probe_bad" data-i18n-attr="onclick:__missing_id"></span>' +
        '<span id="__probe_style" data-i18n-attr="style:__missing_id"></span>';
      document.body.appendChild(host);
      // The guard reports refusals with console.error, which this gate treats
      // as a failure — correctly, in general. Silence it for the duration of
      // the probe only: those two refusals are the behaviour under test, and
      // the page must keep logging them for real operators.
      const realError = console.error;
      console.error = () => {};
      try { applyStatic(host); }
      catch (e) { console.error = realError; return 'applyStatic threw: ' + e.message; }
      finally { console.error = realError; }
      const ok = host.querySelector('#__probe_ok');
      const bad = host.querySelector('#__probe_bad');
      const sty = host.querySelector('#__probe_style');
      const problems = [];
      if (!ok.hasAttribute('title')) problems.push('refused an allowlisted attribute (title)');
      if (bad.hasAttribute('onclick')) problems.push('set onclick= from a catalog id');
      if (sty.hasAttribute('style')) problems.push('set style= from a catalog id');
      host.remove();
      return problems.length ? problems.join('; ') : '';
    })()`);
  if (attrGuard) {
    console.error(`FAIL — ${PAGE_REL} attribute allowlist: ${attrGuard}`);
    proc.kill('SIGKILL');
    process.exit(1);
  }

  const untranslatedByTab = new Map();
  const dataDerived = new Set();
  const hovered = [];
  for (const tab of TABS) {
    // Tabs switch on click. `location.hash` is assigned BY that handler, and
    // the hash-to-click bridge runs once at load, so setting the hash after
    // load silently does nothing and every "tab" measures Overview again.
    const switched = await evaluate(IS_SETUP ? SETUP_STEPS[tab] : `
      (() => {
        const b = document.querySelector('#side nav button[data-tab="${tab}"]');
        if (!b) return false;
        b.click();
        return document.querySelector('#tab-${tab}') ? !document.querySelector('#tab-${tab}').hidden : false;
      })()`);
    if (!switched) {
      console.error(`FAIL — could not reach ${tab} in ${PAGE_REL}; the gate would measure the wrong panels`);
      proc.kill('SIGKILL');
      process.exit(1);
    }
    await sleep(1200);

    // Real pointer input, at the real coordinates of each rendered chart.
    const rects = await evaluate(`
      (() => Array.from(document.querySelectorAll('svg'))
        .map(s => { const r = s.getBoundingClientRect();
          return { id: (s.closest('[id]') || {}).id || '', w: r.width, h: r.height,
                   x: r.left + r.width / 2, y: r.top + r.height / 2 }; })
        .filter(r => r.w > 80 && r.h > 40))()`);

    for (const r of rects) {
      for (const frac of [0.3, 0.5, 0.75]) {
        await browser.send('Input.dispatchMouseEvent', {
          type: 'mouseMoved',
          x: Math.round(r.x + (frac - 0.5) * r.w * 0.8),
          y: Math.round(r.y),
          buttons: 0,
        }, S);
        await sleep(40);
      }
      hovered.push(`${tab}/${r.id}`);
    }

    if (localeArg && localeArg.startsWith('en-X')) {
      // Data is never translated: model ids, their prettified forms, publisher
      // names and client names come from the API, and localizing them would be
      // manipulating data rather than labelling it. Ask the page which strings
      // those are instead of guessing from the fixtures.
      if (IS_SETUP) {
        // The wizard has no API payload to scrape, but it does have inputs —
        // and every data value on screen is one THIS driver typed, plus the
        // secret the stub returned. Naming them keeps the report honest:
        // otherwise the gate calls a masked key and a URL "untranslated" and
        // the count stops meaning anything.
        for (const d of await evaluate(`
          (() => [
            ($('newkey') && $('newkey').value) || '',
            (document.querySelector('#keylist code') || {}).textContent || '',
            ($('baseurl') && $('baseurl').value) || 'https://integrate.api.nvidia.com',
            ($('cbase') && $('cbase').textContent) || '',
            ($('csecret') && $('csecret').textContent) || '',
            'PROXY',
          ].filter(Boolean))()`)) dataDerived.add(d);
      } else {
        for (const d of await evaluate(SCAN_DATA_DERIVED)) dataDerived.add(d);
      }
      // Per tab, so each run is attributed to the tab it was found on. Tab
      // switching only toggles `hidden`, so a later scan would still SEE this
      // markup — but it would blame the wrong tab, and hover tooltips really
      // are transient, so anything hover-only has to be read here or not at all.
      for (const run of await evaluate(SCAN_UNTRANSLATED)) {
        if (!untranslatedByTab.has(run)) untranslatedByTab.set(run, tab);
      }
    }
  }

  // The poll loop re-applies the last hover on every live re-render, which is
  // how a hover throw escalates from "no tooltip" to "the tab stops updating".
  await sleep(3500);

  let doubleEscaped = [];
  if (escapeProbe) {
    doubleEscaped = await evaluate(`
      (() => {
        const bad = [];
        const ENTITY = /&amp;|&#39;|&quot;|&lt;|&gt;/;
        const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
        for (let n = walk.nextNode(); n; n = walk.nextNode()) {
          if (n.parentNode.nodeName === 'SCRIPT' || n.parentNode.nodeName === 'STYLE') continue;
          const t = n.textContent;
          if (ENTITY.test(t)) {
            bad.push({ dir: 'double', text: t.trim().slice(0, 70),
                       el: n.parentNode.className || n.parentNode.nodeName });
          }
        }
        // Attribute sinks. deltaChip's title=, ringGauge's aria-label=,
        // barList/leaderList/segbar title= and applyStatic's setAttribute path
        // are all outside SHOW_TEXT, so a double-escape in any of them shipped
        // green and surfaced as &#39; in a tooltip on the first translation.
        for (const el of document.querySelectorAll('[title],[aria-label],[placeholder],[alt]')) {
          for (const a of ['title', 'aria-label', 'placeholder', 'alt']) {
            const v = el.getAttribute(a);
            if (v && ENTITY.test(v)) {
              bad.push({ dir: 'double', text: v.trim().slice(0, 70),
                         el: (el.className || el.nodeName) + '[' + a + ']' });
            }
          }
        }
        // The other direction: the probe suffix ends in <b>Tag</b>. If a catalog
        // value reached a raw-HTML sink WITHOUT being escaped, that parsed into
        // a real element. Any <b> holding exactly the probe text is a sink that
        // does not escape when it must.
        // The <b> text alone is not enough to accuse: metricRow() and the
        // settings modal both put dynamic values inside <b>, so a value that
        // happened to read "Tag" would be a false accusation. Require the rest
        // of the probe suffix in the same host — it is appended to the SAME
        // catalog value, so an unescaped one always carries both.
        for (const b of document.querySelectorAll('b')) {
          if (b.textContent.trim() !== ${JSON.stringify(PROBE_TAG_TEXT)}) continue;
          const host = b.parentNode;
          const hostText = (host && host.textContent) || '';
          if (!hostText.includes(${JSON.stringify(PROBE_MARKER)})) continue;
          bad.push({ dir: 'missing', text: hostText.trim().slice(0, 70),
                     el: host.className || host.nodeName });
        }
        // Attribute-sink under-escaping: the probe's double quote closed the
        // attribute early, so the parser turned the remainder into attribute
        // NAMES. A real attribute name can never contain <, ", ' or =.
        // (No backticks in this comment — it lives inside a template literal.)
        for (const el of document.querySelectorAll('*')) {
          for (const a of el.getAttributeNames()) {
            if (/[<>"'=]/.test(a)) {
              bad.push({ dir: 'missing',
                         text: 'attribute-name breakout: ' + a.slice(0, 40),
                         el: (el.className || el.nodeName) });
            }
          }
        }
        return bad;
      })()`);
  }

  /* The `status` label is whatever the upstream returned, so "succeeded" is
     not "=== '200'". The captured fixtures only contain 200/429/504/disconnect,
     so replaying them can never observe the disagreement; the predicates
     themselves can, and they are module-scope for exactly this reason. */
  const predicateFailures = IS_SETUP ? [] : await evaluate(`
    (() => {
      if (typeof IS_2XX !== 'function' || typeof IS_ERR !== 'function')
        return ['IS_2XX / IS_ERR are not reachable at module scope'];
      const cases = [
        ['200', true, false], ['201', true, false], ['204', true, false],
        ['299', true, false], ['400', false, true], ['429', false, true],
        ['504', false, true], ['300', false, true], ['2', false, true],
        ['disconnect', false, false], ['stall', false, true],
      ];
      const bad = [];
      for (const [s, ok, err] of cases) {
        if (IS_2XX(s) !== ok) bad.push(\`IS_2XX(\${JSON.stringify(s)}) = \${IS_2XX(s)}, expected \${ok}\`);
        if (IS_ERR(s) !== err) bad.push(\`IS_ERR(\${JSON.stringify(s)}) = \${IS_ERR(s)}, expected \${err}\`);
      }
      return bad;
    })()`);

  const inPage = await evaluate('JSON.stringify(window.__pageErrors || [])').then(JSON.parse);
  for (const e of inPage) {
    const first = (e.stack || '').split('\n').find((l) => /:\d+:\d+/.test(l)) || '';
    const m = first.match(/:(\d+):(\d+)/);
    errors.push({
      text: `${e.kind}: ${e.msg}`,
      line: e.line || (m ? Number(m[1]) : 0),
      col: e.col || (m ? Number(m[2]) : 0),
    });
  }

  proc.kill('SIGKILL');
  await new Promise((r) => proc.on('exit', r));
  // The browser's renderer children can outlive the parent and keep the
  // profile busy. Cleanup is housekeeping: a failure here must never be
  // reported as, or hide, a page result.
  try {
    fs.rmSync(tmpdir, { recursive: true, force: true, maxRetries: 10, retryDelay: 200 });
  } catch (e) {
    console.warn(`note: left ${tmpdir} behind (${e.code})`);
  }

  /* ---------- report ------------------------------------------------------ */

  console.log(IS_SETUP
    ? `drove ${TABS.length} wizard steps in ${PAGE_REL}`
    : `rendered ${TABS.length} tabs, hovered ${hovered.length} charts`);

  if (untranslatedByTab.size) {
    const frozen = frozenTokens();
    // Exact match only. Substring matching against data values excludes real
    // labels: a "Mo" monogram swallowed the heatmap's "Mon", and any run
    // containing "rpm" swallowed "0 / 24 rpm · 3 keys", whose "keys" is ours.
    const isData = (t) => dataDerived.has(t);
    // A run is correctly untranslated only if NOTHING of ours is left once the
    // frozen tokens, digits and punctuation are removed. "tok/s" goes; "24 rpm
    // available" stays, because "available" is a word we wrote.
    const isFrozen = (t) => {
      let rest = t;
      for (const f of [...frozen].sort((a, b) => b.length - a.length)) rest = rest.split(f).join(' ');
      return !/[a-zA-Z]{2,}/.test(rest);
    };
    const real = [...untranslatedByTab].filter(([t]) => !isFrozen(t) && !isData(t));
    if (process.env.DEBUG) {
      const dropped = [...untranslatedByTab].filter(([t]) => isFrozen(t) || isData(t));
      console.log('DEBUG excluded:', JSON.stringify(dropped.map(([t]) => t)));
    }
    const correct = untranslatedByTab.size - real.length;
    console.log(`\nuntranslated runs under ${localeArg}: ${real.length} actionable`
      + ` (${correct} correctly untranslated: frozen tokens and data from the API)`);
    for (const [text, tab] of real) console.log(`   [${tab}] ${JSON.stringify(text)}`);
  }

  if (predicateFailures.length) {
    console.error(`\nFAIL — ${predicateFailures.length} status-predicate disagreement(s)`);
    for (const f of predicateFailures) console.error('  ' + f);
    process.exit(1);
  }

  if (doubleEscaped.length) {
    const dbl = doubleEscaped.filter((d) => d.dir !== 'missing');
    const miss = doubleEscaped.filter((d) => d.dir === 'missing');
    console.error(`\nFAIL — escape-once violated: ${dbl.length} double-escaped, ${miss.length} unescaped`);
    if (dbl.length) console.error('  a helper escaped an already-escaped catalog value:');
    const seen = new Set();
    for (const d of dbl) {
      if (seen.has(d.el)) continue;
      seen.add(d.el);
      console.error(`    .${d.el}  ${JSON.stringify(d.text)}`);
    }
    if (miss.length) console.error('  a catalog value reached a raw-HTML sink unescaped:');
    for (const d of miss) {
      if (seen.has(d.el)) continue;
      seen.add(d.el);
      console.error(`    .${d.el}  ${JSON.stringify(d.text)}`);
    }
    process.exit(1);
  }

  if (errors.length || consoleErrors.length) {
    console.error(`\nFAIL — ${errors.length} uncaught page error(s), ${consoleErrors.length} console error(s)`);
    const seen = new Set();
    for (const e of errors) {
      const key = e.text.split('\n')[0] + ':' + (e.line - LINE_OFFSET);
      if (seen.has(key)) continue;
      seen.add(key);
      console.error(`  ${PAGE_REL}:${Math.max(1, e.line - LINE_OFFSET)}:${e.col}  ${e.text.split('\n')[0]}`);
    }
    for (const c of new Set(consoleErrors)) console.error('  console: ' + c);
    process.exit(1);
  }

  console.log('render ok — no uncaught page errors');
}

main().catch((e) => {
  console.error('render_check failed to run: ' + e.message);
  process.exit(2);
});
