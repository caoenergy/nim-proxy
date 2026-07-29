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
const PAGE = path.join(ROOT, 'src', 'dashboard.html');

const args = process.argv.slice(2);
const localeArg = (() => {
  const i = args.indexOf('--locale');
  return i >= 0 ? args[i + 1] : null;
})();
// Catalog values are escaped once at load, so no render helper may escape its
// label argument again (knowledge/decisions/message-catalog-and-escaping.md).
// English labels contain no escapable character, so a second escape is
// invisible until a real locale ships. This makes it visible now.
const escapeProbe = args.includes('--escape-probe');

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

  const need = ['config.json', 'dashboard.json', 'dashboard-now.json'];
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
    const catalogFile = localeArg === 'en-US'
      ? 'en-US.json'
      : `${localeArg}.json`;
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
  if (escapeProbe) {
    html = html.replace(
      /(<script type="application\/json" id="i18n-catalog">)([\s\S]*?)(<\/script>)/,
      (_m, a, json, b) => {
        const cat = JSON.parse(json);
        for (const k of Object.keys(cat.messages)) {
          const v = cat.messages[k];
          if (typeof v === 'string') cat.messages[k] = v + " Ampersand & Quote'";
          else v.en = v.en + " Ampersand & Quote'";
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

const TABS = ['overview', 'models', 'clients', 'reliability', 'capacity'];

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

  const untranslatedByTab = new Map();
  const hovered = [];
  for (const tab of TABS) {
    // Tabs switch on click. `location.hash` is assigned BY that handler, and
    // the hash-to-click bridge runs once at load, so setting the hash after
    // load silently does nothing and every "tab" measures Overview again.
    const switched = await evaluate(`
      (() => {
        const b = document.querySelector('#side nav button[data-tab="${tab}"]');
        if (!b) return false;
        b.click();
        return document.querySelector('#tab-${tab}') ? !document.querySelector('#tab-${tab}').hidden : false;
      })()`);
    if (!switched) {
      console.error(`FAIL — could not switch to the ${tab} tab; the gate would measure the wrong panels`);
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
      // Per tab: panels are torn down and rebuilt on tab switch, so a single
      // scan after the loop only ever sees the last tab.
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
        const walk = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
        for (let n = walk.nextNode(); n; n = walk.nextNode()) {
          if (n.parentNode.nodeName === 'SCRIPT' || n.parentNode.nodeName === 'STYLE') continue;
          const t = n.textContent;
          if (/&amp;|&#39;|&quot;|&lt;|&gt;/.test(t)) {
            bad.push({ text: t.trim().slice(0, 70), el: n.parentNode.className || n.parentNode.nodeName });
          }
        }
        return bad;
      })()`);
  }

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

  console.log(`rendered ${TABS.length} tabs, hovered ${hovered.length} charts`);

  if (untranslatedByTab.size) {
    const frozen = frozenTokens();
    const isFrozen = (t) => frozen.some((f) => t === f || t.includes(f));
    const real = [...untranslatedByTab].filter(([t]) => !isFrozen(t));
    const correct = untranslatedByTab.size - real.length;
    console.log(`\nuntranslated runs under ${localeArg}: ${real.length} actionable`
      + ` (${correct} correctly untranslated: frozen tokens, model ids, client names)`);
    for (const [text, tab] of real) console.log(`   [${tab}] ${JSON.stringify(text)}`);
  }

  if (doubleEscaped.length) {
    console.error(`\nFAIL — ${doubleEscaped.length} double-escaped run(s): a helper escaped an already-escaped catalog value`);
    const seen = new Set();
    for (const d of doubleEscaped) {
      if (seen.has(d.el)) continue;
      seen.add(d.el);
      console.error(`  .${d.el}  ${JSON.stringify(d.text)}`);
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
      console.error(`  src/dashboard.html:${Math.max(1, e.line - LINE_OFFSET)}:${e.col}  ${e.text.split('\n')[0]}`);
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
