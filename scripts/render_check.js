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
const vm = require('vm');
const net = require('net');
const { spawn, execFileSync } = require('child_process');

const ROOT = path.resolve(__dirname, '..');
const FIXTURES = path.join(ROOT, 'tests', 'fixtures', 'api');

const args = process.argv.slice(2);
const PRESENTATION_CSP = "default-src 'none'; img-src 'self' data:; style-src 'self'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'";

const ASSET_CASES = [
  {
    name: 'external-script',
    files: { 'dashboard.html': '<script src="https://cdn.invalid/app.js"></script>' },
    want: 'external-script',
  },
  {
    name: 'external-stylesheet-font',
    files: {
      'dashboard.html': '<link rel="stylesheet" href="https://fonts.invalid/ui.css">',
      'operator.css': '@font-face { src: url(https://fonts.invalid/ui.woff2); }',
    },
    want: 'external-stylesheet-font',
  },
  {
    name: 'external-image',
    files: { 'dashboard.html': '<img src="https://images.invalid/model.svg">' },
    want: 'external-image',
  },
  {
    name: 'external-css-url',
    files: { 'operator.css': '.logo { background: url(http://images.invalid/logo.svg); }' },
    want: 'external-css-url',
  },
  {
    name: 'external-script-protocol-relative-unquoted',
    files: { 'dashboard.html': '<script defer src=//cdn.invalid/app.js></script>' },
    want: 'external-script',
  },
  {
    name: 'external-stylesheet-reordered-attributes',
    files: {
      'dashboard.html': '<link href="//fonts.invalid/ui.css" media="screen" rel="stylesheet">',
    },
    want: 'external-stylesheet-font',
  },
  {
    name: 'external-image-srcset-protocol-relative',
    files: {
      'dashboard.html': '<img alt="model" srcset="//images.invalid/model.svg 1x, /local.svg 2x">',
    },
    want: 'external-image',
  },
  {
    name: 'external-css-quoted-import',
    files: { 'operator.css': '@import "//fonts.invalid/ui.css";' },
    want: 'external-stylesheet-font',
  },
  {
    name: 'external-css-url-protocol-relative',
    files: { 'operator.css': '.logo { background: url("//images.invalid/logo.svg"); }' },
    want: 'external-css-url',
  },
  {
    name: 'local-only',
    files: {
      'dashboard.html': '<link rel="stylesheet" href="/assets/operator/operator.css"><script src="/assets/operator/dashboard.js"></script>',
      'operator.css': '.logo { background: none; }',
    },
    want: null,
  },
];

function isExternalUrl(value) {
  return /^(?:https?:)?\/\//i.test(String(value || '').trim());
}

function srcsetHasExternalUrl(value) {
  return String(value || '').split(',').some(candidate =>
    isExternalUrl(candidate.trim().split(/\s+/, 1)[0]));
}

/* This is deliberately a small context parser, not a spelling regex. It
   tokenizes start tags and attributes so attribute order, quote style and
   unquoted values cannot change what resource-loading context is inspected. */
function htmlElements(source) {
  const elements = [];
  const tags = /<([A-Za-z][A-Za-z0-9:-]*)(\s(?:[^"'<>]|"[^"]*"|'[^']*')*)?>/g;
  for (const match of source.matchAll(tags)) {
    const attrs = new Map();
    const attrSource = match[2] || '';
    const attributes = /([^\s"'=<>`]+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g;
    for (const attr of attrSource.matchAll(attributes)) {
      attrs.set(attr[1].toLowerCase(), attr[2] ?? attr[3] ?? attr[4] ?? '');
    }
    elements.push({
      tag: match[1].toLowerCase(),
      attrs,
      end: (match.index || 0) + match[0].length,
    });
  }
  return elements;
}

function cssUrls(source) {
  const urls = [];
  const pattern = /url\(\s*(?:"([^"]*)"|'([^']*)'|([^)\s]+))\s*\)/gi;
  for (const match of source.matchAll(pattern)) {
    urls.push(match[1] ?? match[2] ?? match[3] ?? '');
  }
  return urls;
}

function cssImports(source) {
  const urls = [];
  const pattern = /@import\s+(?:url\(\s*)?(?:"([^"]*)"|'([^']*)'|([^;\s)]+))/gi;
  for (const match of source.matchAll(pattern)) {
    urls.push(match[1] ?? match[2] ?? match[3] ?? '');
  }
  return urls;
}

function assetProblems(files) {
  const problems = [];
  for (const [filename, source] of Object.entries(files)) {
    const markup = filename.endsWith('.html') || filename.endsWith('.svg');
    if (markup) {
      for (const element of htmlElements(source)) {
        const { tag, attrs } = element;
        if (tag === 'script' && isExternalUrl(attrs.get('src'))) {
          problems.push({ check: 'external-script', detail: filename });
        }
        if (tag === 'link'
            && isExternalUrl(attrs.get('href'))
            && attrs.get('rel')?.toLowerCase().split(/\s+/)
              .some(rel => ['stylesheet', 'preconnect', 'dns-prefetch', 'icon'].includes(rel))) {
          problems.push({ check: 'external-stylesheet-font', detail: filename });
        }
        if (['img', 'image', 'source', 'video'].includes(tag)) {
          const direct = ['src', 'href', 'xlink:href', 'poster']
            .some(attr => isExternalUrl(attrs.get(attr)));
          if (direct || srcsetHasExternalUrl(attrs.get('srcset'))) {
            problems.push({ check: 'external-image', detail: filename });
          }
        }
        if (!filename.endsWith('.html')) continue;
        if (tag === 'style' || attrs.has('style')) {
          problems.push({ check: 'inline-style', detail: filename });
        }
        if ([...attrs.keys()].some(attr => /^on[a-z]+$/i.test(attr))) {
          problems.push({ check: 'inline-event-handler', detail: filename });
        }
        if (tag === 'script' && !attrs.has('src')
            && attrs.get('type')?.toLowerCase() !== 'application/json') {
          const closing = source.toLowerCase().indexOf('</script', element.end);
          if (closing < 0 || source.slice(element.end, closing).trim()) {
            problems.push({ check: 'inline-script', detail: filename });
          }
        }
      }
    }
    if (filename.endsWith('.css')) {
      const importsExternal = cssImports(source).some(isExternalUrl);
      const fontBlocks = [...source.matchAll(/@font-face\s*\{([^}]*)\}/gi)]
        .some(match => cssUrls(match[1]).some(isExternalUrl));
      if (importsExternal || fontBlocks) {
        problems.push({ check: 'external-stylesheet-font', detail: filename });
      }
      if (cssUrls(source).some(isExternalUrl) && !fontBlocks) {
        problems.push({ check: 'external-css-url', detail: filename });
      }
    }
  }
  return problems;
}

function assetSelftest() {
  const failures = [];
  for (const { name, files, want } of ASSET_CASES) {
    const problems = assetProblems(files);
    const got = problems[0] ? problems[0].check : null;
    if (got !== want) {
      failures.push(`${name}: expected check ${want || 'nothing'}, got ${got || 'nothing'}`);
    } else {
      console.log(`  ok  ${name}: ${got || 'no problem'}`);
    }
  }
  if (failures.length) {
    for (const failure of failures) console.error(failure);
    return 1;
  }
  console.log('asset selftest ok — every external-origin check observed');
  return 0;
}

function sourceFilesUnder(dir) {
  if (!fs.existsSync(dir)) return [];
  const out = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...sourceFilesUnder(full));
    else out.push(full);
  }
  return out;
}

function assetsOnly() {
  const web = path.join(ROOT, 'src', 'web');
  const paths = sourceFilesUnder(web);
  if (!paths.length) {
    console.error('[asset-directory] src/web has no split presentation sources');
    return 1;
  }
  const files = Object.fromEntries(paths.map((filename) => [
    path.relative(web, filename),
    fs.readFileSync(filename, 'utf8'),
  ]));
  const problems = assetProblems(files);
  if (problems.length) {
    for (const { check, detail } of problems) console.error(`[${check}] ${detail}`);
    return 1;
  }
  console.log(`presentation assets OK — ${paths.length} local sources; no external origins or inline executable/style contexts`);
  return 0;
}

if (args.includes('--asset-selftest')) process.exit(assetSelftest());
if (args.includes('--assets-only')) process.exit(assetsOnly());

const SYNTAX_CASES = [
  { name: 'valid', html: '<script>const ok = 1;</script>', want: null },
  { name: 'invalid', html: '<script>const = ;</script>', want: 'syntax' },
  { name: 'missing', html: '<main></main>', want: 'script-block' },
  {
    name: 'multiple',
    html: '<script>const a = 1;</script><script>const b = 2;</script>',
    want: 'script-block',
  },
];

function syntaxProblems(html, filename) {
  const blocks = [...html.matchAll(/<script>(?:\r?\n)?([\s\S]*?)<\/script>/g)];
  if (blocks.length !== 1) {
    return [{
      check: 'script-block',
      detail: `${filename}: expected one plain <script>, found ${blocks.length}`,
    }];
  }
  try {
    new vm.Script(blocks[0][1], { filename });
    return [];
  } catch (err) {
    return [{ check: 'syntax', detail: `${filename}: ${err.message}` }];
  }
}

function syntaxSelftest() {
  const failures = [];
  for (const { name, html, want } of SYNTAX_CASES) {
    const problems = syntaxProblems(html, `${name}.html`);
    const got = problems[0] ? problems[0].check : null;
    if (got !== want) {
      failures.push(`${name}: expected check ${want || 'nothing'}, got ${got || 'nothing'}`);
    } else {
      console.log(`  ok  ${name}: ${got || 'no problem'}`);
    }
  }
  if (failures.length) {
    for (const failure of failures) console.error(failure);
    return 1;
  }
  console.log('syntax selftest ok — every check observed');
  return 0;
}

if (args.includes('--syntax-selftest')) process.exit(syntaxSelftest());

function syntaxOnly() {
  const scripts = ['shared.js', 'dashboard.js', 'settings.js', 'setup.js', 'login.js'];
  const problems = [];
  for (const script of scripts) {
    const filename = path.join('src', 'web', script);
    const full = path.join(ROOT, filename);
    if (!fs.existsSync(full)) {
      problems.push({ check: 'script-file', detail: `${filename}: missing` });
      continue;
    }
    try {
      new vm.Script(fs.readFileSync(full, 'utf8'), { filename });
    } catch (err) {
      problems.push({ check: 'syntax', detail: `${filename}: ${err.message}` });
    }
  }
  if (problems.length) {
    for (const { check, detail } of problems) console.error(`[${check}] ${detail}`);
    return 1;
  }
  console.log('presentation script syntax OK — all split sources parse');
  return 0;
}

if (args.includes('--syntax-only')) process.exit(syntaxOnly());

function servedPageSelftest() {
  const failures = [];
  const runSource = String(main);
  if (typeof probeCatalogHtml !== 'function') {
    failures.push('catalog probe does not accept server-owned HTML');
  } else if (/readFileSync\s*\(\s*PAGE\b/.test(String(probeCatalogHtml))) {
    failures.push('catalog probe still reads a private page source');
  }
  if (!runSource.includes("'Fetch.getResponseBody'")) {
    failures.push('probe does not read the real response body');
  }
  if (!runSource.includes('serverPageResponses.add')) {
    failures.push('probe does not record server response provenance');
  }
  if (runSource.includes('probePageHtml()')) {
    failures.push('test-only page assembly remains reachable');
  }
  if (failures.length) {
    for (const failure of failures) console.error(`[served-page] ${failure}`);
    return 1;
  }
  console.log('served-page selftest ok — probe derives from and tracks the real response');
  return 0;
}

if (args.includes('--served-page-selftest')) process.exit(servedPageSelftest());

function renderTempdirs() {
  return new Set(fs.readdirSync(os.tmpdir())
    .filter(name => name.startsWith('nimproxy-render-'))
    .map(name => path.join(os.tmpdir(), name)));
}

function processDescendants(rootPid) {
  if (process.platform === 'win32') return [];
  const rows = execFileSync('ps', ['-eo', 'pid=,ppid='], { encoding: 'utf8' })
    .trim().split('\n').map(line => line.trim().split(/\s+/).map(Number));
  const found = [];
  const pending = [rootPid];
  while (pending.length) {
    const parent = pending.pop();
    for (const [pid, ppid] of rows) {
      if (ppid !== parent || found.includes(pid)) continue;
      found.push(pid);
      pending.push(pid);
    }
  }
  return found;
}

async function cleanupSelftest() {
  const realChrome = findChrome();
  if (!realChrome) {
    console.error('cleanup selftest needs Chromium');
    return 2;
  }
  const probeRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'nimproxy-cleanup-probe-'));
  const wrapper = path.join(probeRoot, 'chrome-wrapper.js');
  const pidFile = path.join(probeRoot, 'pids.json');
  const writerSource = [
    '"use strict";',
    'const fs = require("fs");',
    'const path = require("path");',
    'const profile = process.argv[1];',
    'let n = 0;',
    'setInterval(() => {',
    '  try {',
    '    fs.mkdirSync(profile, { recursive: true });',
    '    fs.writeFileSync(path.join(profile, "cleanup-race-" + (n++ % 8)), "x");',
    '  } catch (_) {}',
    '}, 5);',
  ].join('\n');
  const wrapperSource = [
    '#!/usr/bin/env node',
    '"use strict";',
    'const fs = require("fs");',
    'const { spawn } = require("child_process");',
    'const args = process.argv.slice(2);',
    'const profileArg = args.find(value => value.startsWith("--user-data-dir="));',
    'const profile = profileArg.slice("--user-data-dir=".length);',
    'const chrome = spawn(process.env.NIMPROXY_REAL_CHROME, args,',
    '  { stdio: ["ignore", "pipe", "pipe"] });',
    'chrome.stdout.pipe(process.stdout);',
    'chrome.stderr.pipe(process.stderr);',
    `const writer = spawn(process.execPath, ["-e", ${JSON.stringify(writerSource)}, profile],`,
    '  { stdio: "ignore" });',
    'fs.writeFileSync(process.env.NIMPROXY_CLEANUP_PID_FILE,',
    '  JSON.stringify({ chrome: chrome.pid, writer: writer.pid }));',
    'chrome.on("exit", code => { process.exitCode = code || 0; });',
  ].join('\n');
  fs.writeFileSync(wrapper, wrapperSource, { mode: 0o700 });

  const before = renderTempdirs();
  let stdout = '';
  let stderr = '';
  const child = spawn(process.execPath, [__filename, '--page', 'setup'], {
    cwd: ROOT,
    env: {
      ...process.env,
      CHROME: wrapper,
      NIMPROXY_CLEANUP_PID_FILE: pidFile,
      NIMPROXY_REAL_CHROME: realChrome,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  child.stdout.on('data', chunk => { stdout += chunk.toString(); });
  child.stderr.on('data', chunk => { stderr += chunk.toString(); });
  const result = await Promise.race([
    new Promise(resolve => child.once('exit', code => resolve({ code, timedOut: false }))),
    sleep(30000).then(() => ({ code: null, timedOut: true })),
  ]);
  if (result.timedOut) child.kill('SIGKILL');
  await sleep(300);
  const leaked = [...renderTempdirs()].filter(dir => !before.has(dir));

  let pids = [];
  try {
    const recorded = JSON.parse(fs.readFileSync(pidFile, 'utf8'));
    pids = [recorded.chrome, recorded.writer].filter(Number.isInteger);
  } catch (_) {}
  const descendants = pids.flatMap(processDescendants);
  for (const pid of [...new Set([...descendants.reverse(), ...pids])]) {
    try { process.kill(pid, 'SIGKILL'); } catch (_) {}
  }
  await sleep(300);
  for (const dir of leaked) {
    fs.rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }

  const failures = [];
  if (result.timedOut) failures.push('render process stayed alive after reporting its result');
  else if (result.code !== 0) failures.push(`render process exited ${result.code}`);
  if (leaked.length) failures.push(`left ${leaked.length} nimproxy-render temp director${leaked.length === 1 ? 'y' : 'ies'}`);
  if (/note: left .*nimproxy-render-/.test(stderr)) failures.push('cleanup failure was downgraded to a note');

  const startupMarker = path.join(probeRoot, 'proxy-exit.json');
  const startupBefore = renderTempdirs();
  let startupStderr = '';
  const startup = spawn(process.execPath, [__filename, '--page', 'setup'], {
    cwd: ROOT,
    env: {
      ...process.env,
      CHROME: realChrome,
      NIMPROXY_RENDER_HEALTH_PATH: '/cleanup-selftest-never-ready',
      NIMPROXY_RENDER_HEALTH_TIMEOUT_MS: '200',
      NIMPROXY_RENDER_PROXY_EXIT_MARKER: startupMarker,
    },
    stdio: ['ignore', 'ignore', 'pipe'],
  });
  startup.stderr.on('data', chunk => { startupStderr += chunk.toString(); });
  const startupResult = await Promise.race([
    new Promise(resolve => startup.once('exit', code => resolve({ code, timedOut: false }))),
    sleep(10000).then(() => ({ code: null, timedOut: true })),
  ]);
  if (startupResult.timedOut) startup.kill('SIGKILL');
  await sleep(100);
  const startupLeaked = [...renderTempdirs()].filter(dir => !startupBefore.has(dir));
  let startupExit = null;
  try {
    startupExit = JSON.parse(fs.readFileSync(startupMarker, 'utf8'));
  } catch (_) {}
  if (startupResult.timedOut) failures.push('startup-timeout render process stayed alive');
  else if (startupResult.code !== 2) failures.push(`startup-timeout render process exited ${startupResult.code}`);
  if (!startupStderr.includes('proxy did not become healthy')) {
    failures.push('startup-timeout did not report the health failure');
  }
  if (!startupExit?.runDirectoryExisted) {
    failures.push('startup-timeout removed the run directory before the proxy exit was observed');
  }
  if (startupLeaked.length) {
    failures.push(`startup-timeout left ${startupLeaked.length} nimproxy-render temp director${startupLeaked.length === 1 ? 'y' : 'ies'}`);
  }

  const localeBefore = renderTempdirs();
  let localeStderr = '';
  const locale = spawn(
    process.execPath,
    [__filename, '--locale', 'cleanup-selftest-missing'],
    {
      cwd: ROOT,
      env: { ...process.env, CHROME: realChrome },
      stdio: ['ignore', 'ignore', 'pipe'],
    },
  );
  locale.stderr.on('data', chunk => { localeStderr += chunk.toString(); });
  const localeResult = await Promise.race([
    new Promise(resolve => locale.once('exit', code => resolve({ code, timedOut: false }))),
    sleep(30000).then(() => ({ code: null, timedOut: true })),
  ]);
  if (localeResult.timedOut) locale.kill('SIGKILL');
  await sleep(100);
  const localeLeaked = [...renderTempdirs()].filter(dir => !localeBefore.has(dir));
  if (localeResult.timedOut) failures.push('missing-locale render process stayed alive');
  else if (localeResult.code !== 1) failures.push(`missing-locale render process exited ${localeResult.code}`);
  if (!localeStderr.includes('missing locale locales/cleanup-selftest-missing.json')) {
    failures.push('missing-locale failure hid its originating diagnostic');
  }
  if (localeLeaked.length) {
    failures.push(`missing-locale left ${localeLeaked.length} nimproxy-render temp director${localeLeaked.length === 1 ? 'y' : 'ies'}`);
  }

  for (const dir of [...startupLeaked, ...localeLeaked]) {
    fs.rmSync(dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
  fs.rmSync(probeRoot, { recursive: true, force: true });

  if (failures.length) {
    for (const failure of failures) console.error(`[cleanup] ${failure}`);
    if (stderr.trim()) console.error(stderr.trim());
    return 1;
  }
  console.log('cleanup selftest ok — browser descendants stopped and run directory removed');
  return 0;
}
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
const PAGE_REL = path.join('src', 'web', `${pageArg}.html`);
const CATALOG_PREFIX = IS_SETUP ? 'setup-' : '';
const localeArg = (() => {
  const i = args.indexOf('--locale');
  return i >= 0 ? args[i + 1] : null;
})();
// Catalog values are plain Unicode text. The probe proves ordinary text and
// allowed attributes render literally, while fixed-markup HTML builders
// perform their one context-appropriate escape at the sink.
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

/* ---------- production server and captured API responses ------------------ */

function loadFixtures() {
  const need = IS_SETUP ? [] : ['config.json', 'dashboard.json', 'dashboard-now.json'];
  for (const f of need) {
    const p = path.join(FIXTURES, f);
    if (!fs.existsSync(p)) {
      const error = new Error(`missing fixture ${path.relative(ROOT, p)} — see ${path.relative(ROOT, path.join(FIXTURES, 'README.md'))}`);
      error.exitCode = 2;
      throw error;
    }
  }
  return Object.fromEntries(
    need.map((f) => [f, JSON.parse(fs.readFileSync(path.join(FIXTURES, f), 'utf8'))]),
  );
}

function probeCatalogHtml(serverHtml) {
  let html = serverHtml;
  if (localeArg) {
    const catalogFile = `${CATALOG_PREFIX}${localeArg}.json`;
    const cp = path.join(ROOT, 'locales', catalogFile);
    if (!fs.existsSync(cp)) {
      const error = new Error(`missing locale ${path.relative(ROOT, cp)}`);
      error.exitCode = 2;
      throw error;
    }
    const cat = fs.readFileSync(cp, 'utf8').trim();
    html = html.replace(
      /(<script type="application\/json" id="i18n-catalog">)[\s\S]*?(<\/script>)/,
      (_m, a, b) => a + cat + b,
    );
  }
  if (escapeProbe) {
    html = html.replace(
      /(<script type="application\/json" id="i18n-catalog">)([\s\S]*?)(<\/script>)/,
      (_m, a, json, b) => {
        const cat = JSON.parse(json);
        for (const key of Object.keys(cat.messages)) {
          const value = cat.messages[key];
          if (typeof value === 'string') cat.messages[key] = value + PROBE_SUFFIX;
          else value.en += PROBE_SUFFIX;
        }
        return a + JSON.stringify(cat) + b;
      },
    );
  }
  const withoutCatalog = value => value.replace(
    /(<script type="application\/json" id="i18n-catalog">)[\s\S]*?(<\/script>)/,
    '$1[PROBE CATALOG]$2',
  );
  if (withoutCatalog(html) !== withoutCatalog(serverHtml)) {
    throw new Error('probe changed server HTML outside the catalog body');
  }
  return html;
}

function configuredStore() {
  return {
    version: 1,
    upstream: {
      base_url: 'http://127.0.0.1:9',
      nim_keys: [{ key: 'render-fixture-key', owner: 'root', enabled: true, rpm: 40 }],
    },
    client_auth: { mode: 'open', keys: [] },
    limits: { heartbeat_secs: 1 },
    users: [{
      username: 'root',
      password_hash: 'pbkdf2-sha256$1000$00000000000000000000000000000000$dd5fe0be04ca7f9e24642561a5d4635c52c40be82cbd7587b5eddc913ad3c7a7',
      role: 'superuser',
    }],
  };
}

async function freePort() {
  return await new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const port = server.address().port;
      server.close((error) => error ? reject(error) : resolve(port));
    });
  });
}

async function startProxy(tmpdir) {
  const binary = path.join(ROOT, 'target', 'debug', 'nim-proxy');
  execFileSync('cargo', ['build', '--quiet', '--bin', 'nim-proxy'], {
    cwd: ROOT,
    stdio: 'inherit',
  });
  const dataDir = path.join(tmpdir, 'data');
  fs.mkdirSync(dataDir);
  if (!IS_SETUP) {
    fs.writeFileSync(
      path.join(dataDir, 'config.json'),
      JSON.stringify(configuredStore(), null, 2),
    );
  }
  const port = await freePort();
  const proc = spawn(binary, [], {
    cwd: tmpdir,
    env: {
      DATA_DIR: dataDir,
      HISTORY_SAMPLE_SECS: '3600',
      HOST: '127.0.0.1',
      PORT: String(port),
      RUST_LOG: 'nim_proxy=warn',
    },
    stdio: ['ignore', 'ignore', 'pipe'],
  });
  if (process.env.NIMPROXY_RENDER_PROXY_EXIT_MARKER) {
    proc.once('exit', () => {
      fs.writeFileSync(
        process.env.NIMPROXY_RENDER_PROXY_EXIT_MARKER,
        JSON.stringify({
          pid: proc.pid,
          runDirectoryExisted: fs.existsSync(tmpdir),
        }),
      );
    });
  }
  const origin = `http://127.0.0.1:${port}`;
  let stderr = '';
  proc.stderr.on('data', chunk => { stderr += chunk.toString(); });
  const healthPath = process.env.NIMPROXY_RENDER_HEALTH_PATH || '/health';
  const healthTimeoutMs = Number(process.env.NIMPROXY_RENDER_HEALTH_TIMEOUT_MS || 20000);
  const deadline = Date.now() + healthTimeoutMs;
  while (Date.now() < deadline) {
    if (proc.exitCode !== null) throw new Error(`proxy exited early: ${proc.exitCode}\n${stderr}`);
    try {
      const response = await fetch(origin + healthPath);
      if (response.ok) return { proc, origin };
    } catch (_) {}
    await sleep(50);
  }
  proc.kill('SIGKILL');
  throw new Error(`proxy did not become healthy\n${stderr}`);
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
    // Toggle both ways: apiAccessMessageId() has two branches and both feed the
    // ID-taking text sink. Leave it checked so step4 reaches showConnect
    // rather than navigating to /.
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

function waitForProcessExit(proc, timeoutMs) {
  if (!proc || proc.exitCode !== null || proc.signalCode !== null) return Promise.resolve(true);
  return new Promise(resolve => {
    let settled = false;
    const finish = value => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      proc.removeListener('exit', onExit);
      resolve(value);
    };
    const onExit = () => finish(true);
    const timer = setTimeout(() => finish(false), timeoutMs);
    proc.once('exit', onExit);
    if (proc.exitCode !== null || proc.signalCode !== null) finish(true);
  });
}

function browserGroupAlive(pid) {
  if (!pid || process.platform === 'win32') return false;
  try {
    process.kill(-pid, 0);
    return true;
  } catch (error) {
    return error.code !== 'ESRCH';
  }
}

async function waitForBrowserGroupExit(pid, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (browserGroupAlive(pid) && Date.now() < deadline) await sleep(25);
  return !browserGroupAlive(pid);
}

async function shutdownRun(browser, browserProc, proxyProc, tmpdir) {
  const failures = [];
  if (browser) {
    try {
      await Promise.race([
        browser.send('Browser.close'),
        sleep(2000).then(() => { throw new Error('Browser.close timed out'); }),
      ]);
    } catch (_) {
      // The socket commonly closes before CDP can acknowledge Browser.close.
      // Process and group exit below are the authoritative result.
    }
  }

  const browserExited = await waitForProcessExit(browserProc, 2000);
  let browserTreeExited = process.platform === 'win32'
    ? browserExited
    : await waitForBrowserGroupExit(browserProc?.pid, browserExited ? 250 : 0);
  if (!browserExited || !browserTreeExited) {
    try {
      if (process.platform === 'win32') browserProc?.kill('SIGKILL');
      else if (browserProc?.pid) process.kill(-browserProc.pid, 'SIGKILL');
    } catch (error) {
      if (error.code !== 'ESRCH') failures.push(`browser termination: ${error.message}`);
    }
    const forcedParentExit = await waitForProcessExit(browserProc, 2000);
    browserTreeExited = process.platform === 'win32'
      ? forcedParentExit
      : await waitForBrowserGroupExit(browserProc?.pid, 2000);
    if (!forcedParentExit || !browserTreeExited) {
      failures.push('browser process tree did not exit');
    }
  }
  try { browser?.ws.close(); } catch (_) {}

  if (proxyProc && proxyProc.exitCode === null && proxyProc.signalCode === null) {
    proxyProc.kill('SIGTERM');
    if (!await waitForProcessExit(proxyProc, 2000)) {
      proxyProc.kill('SIGKILL');
      if (!await waitForProcessExit(proxyProc, 2000)) failures.push('proxy process did not exit');
    }
  }

  if (tmpdir) {
    try {
      fs.rmSync(tmpdir, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
    } catch (error) {
      failures.push(`remove ${tmpdir}: ${error.code || error.message}`);
    }
    if (fs.existsSync(tmpdir)) failures.push(`run directory still exists: ${tmpdir}`);
  }
  if (failures.length) throw new Error(`render cleanup failed: ${failures.join('; ')}`);
}

function reportedFailure() {
  const error = new Error('render proof failed');
  error.reported = true;
  error.exitCode = 1;
  return error;
}

async function main() {
  const chrome = findChrome();
  if (!chrome) {
    const error = new Error('no chromium found. Set CHROME=/path/to/chrome.');
    error.exitCode = 2;
    throw error;
  }

  const fixtures = loadFixtures();
  const tmpdir = fs.mkdtempSync(path.join(os.tmpdir(), 'nimproxy-render-'));
  let proxy = null;
  let browserProc = null;
  let browser = null;
  let cleanupPromise = null;
  const cleanupRun = () => {
    if (!cleanupPromise) {
      cleanupPromise = shutdownRun(browser, browserProc, proxy?.proc, tmpdir);
    }
    return cleanupPromise;
  };
  try {
    proxy = await startProxy(tmpdir);

    browserProc = spawn(chrome, [
    '--headless=new',
    '--disable-gpu',
    '--no-sandbox',
    '--hide-scrollbars',
    '--window-size=1440,2400',
    '--remote-debugging-port=0',
    `--user-data-dir=${path.join(tmpdir, 'profile')}`,
    'about:blank',
    ], {
      detached: process.platform !== 'win32',
      stdio: ['ignore', 'ignore', 'pipe'],
    });

  const wsUrl = await new Promise((resolve, reject) => {
    let buf = '';
    const timer = setTimeout(() => reject(new Error('browser did not report a debug port')), 20000);
    browserProc.stderr.on('data', (d) => {
      buf += d.toString();
      const m = buf.match(/ws:\/\/[^\s]+/);
      if (m) {
        clearTimeout(timer);
        resolve(m[0]);
      }
    });
    browserProc.on('exit', (c) => reject(new Error('browser exited early: ' + c)));
  });

  browser = await CDP.connect(wsUrl);
  const { targetId } = await browser.send('Target.createTarget', { url: 'about:blank' });
  const { sessionId } = await browser.send('Target.attachToTarget', { targetId, flatten: true });
  const S = sessionId;

  const errors = [];
  const consoleErrors = [];
  const requestUrls = new Map();
  const initialAssetErrors = [];
  const requestedPresentation = new Set();
  const servedPresentation = new Set();
  const serverPageResponses = new Set();
  const fetchOperations = new Set();
  const fetched = [];
  const pagePath = IS_SETUP ? '/setup' : '/';
  const presentationPath = value =>
    value === '/' || value === '/setup' || value.startsWith('/assets/');
  const headerValue = (headers, name) =>
    (headers || []).find(header => header.name.toLowerCase() === name)?.value;
  async function handleFetchPaused(params) {
    const { request } = params;
    const url = new URL(request.url);
    if (params.responseStatusCode !== undefined) {
      const headers = params.responseHeaders || [];
      if (url.pathname !== pagePath || request.method !== 'GET') {
        throw new Error(`unexpected response-stage interception: ${request.method} ${url.pathname}`);
      }
      if (params.responseStatusCode !== 200) {
        throw new Error(`server page response was ${params.responseStatusCode} ${url.pathname}`);
      }
      if (headerValue(headers, 'content-type') !== 'text/html; charset=utf-8'
          || headerValue(headers, 'cache-control') !== 'no-store'
          || headerValue(headers, 'content-security-policy') !== PRESENTATION_CSP) {
        throw new Error(`server page response headers violated the presentation contract: ${url.pathname}`);
      }
      const response = await browser.send(
        'Fetch.getResponseBody',
        { requestId: params.requestId },
        S,
      );
      const serverHtml = response.base64Encoded
        ? Buffer.from(response.body, 'base64').toString('utf8')
        : response.body;
      const body = probeCatalogHtml(serverHtml);
      serverPageResponses.add(url.pathname);
      await browser.send('Fetch.fulfillRequest', {
        requestId: params.requestId,
        responseCode: params.responseStatusCode,
        responseHeaders: headers.filter(header =>
          !['content-length', 'content-encoding', 'transfer-encoding']
            .includes(header.name.toLowerCase())),
        body: Buffer.from(body).toString('base64'),
      }, S);
      return;
    }
    let body = null;
    if (url.pathname === '/api/config') body = fixtures['config.json'];
    else if (url.pathname === '/api/dashboard/now') body = fixtures['dashboard-now.json'];
    else if (url.pathname === '/api/dashboard') body = fixtures['dashboard.json'];
    else if (url.pathname === '/setup/validate-key' && request.method === 'POST')
      body = { ok: true, models: 63 };
    else if (url.pathname === '/setup' && request.method === 'POST')
      body = { ok: true, client_key: { name: 'default', secret: 'npk_probe_secret' } };
    if (body !== null) {
      fetched.push(url.pathname);
      await browser.send('Fetch.fulfillRequest', {
        requestId: params.requestId,
        responseCode: 200,
        responseHeaders: [{ name: 'Content-Type', value: 'application/json' }],
        body: Buffer.from(JSON.stringify(body)).toString('base64'),
      }, S);
    } else {
      await browser.send('Fetch.continueRequest', { requestId: params.requestId }, S);
    }
  }
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
      const entry = msg.params.entry;
      consoleErrors.push('[' + entry.source + '] ' + entry.text
        + (entry.url ? ` (${entry.url})` : ''));
    }
    if (msg.method === 'Runtime.consoleAPICalled' && msg.params.type === 'error') {
      consoleErrors.push(msg.params.args.map((a) => a.value ?? a.description).join(' '));
    }
    if (msg.method === 'Network.requestWillBeSent') {
      const url = msg.params.request.url;
      requestUrls.set(msg.params.requestId, url);
      if (url.startsWith('http') && !url.startsWith(proxy.origin)) {
        initialAssetErrors.push(`external request ${url}`);
      }
      if (url.startsWith(proxy.origin)) {
        const pathname = new URL(url).pathname;
        if (presentationPath(pathname)) requestedPresentation.add(pathname);
      }
    }
    if (msg.method === 'Network.responseReceived') {
      const { response } = msg.params;
      const url = new URL(response.url);
      if (response.url.startsWith(proxy.origin)
          && (url.pathname === '/' || url.pathname === '/setup' || url.pathname.startsWith('/assets/'))
          && response.status >= 400) {
        initialAssetErrors.push(`${response.status} ${url.pathname}`);
      } else if (response.url.startsWith(proxy.origin)
          && (url.pathname === '/' || url.pathname === '/setup' || url.pathname.startsWith('/assets/'))) {
        servedPresentation.add(url.pathname);
      }
    }
    if (msg.method === 'Network.loadingFailed') {
      const url = requestUrls.get(msg.params.requestId) || 'unknown request';
      if (url.startsWith(proxy.origin)) {
        initialAssetErrors.push(`${msg.params.errorText} ${url}`);
      }
    }
    if (msg.method === 'Fetch.requestPaused') {
      const operation = handleFetchPaused(msg.params)
        .catch(async error => {
          errors.push({ text: error.message, line: 0, col: 0 });
          try {
            await browser.send('Fetch.failRequest', {
              requestId: msg.params.requestId,
              errorReason: 'Failed',
            }, S);
          } catch (_) {}
        })
        .finally(() => fetchOperations.delete(operation));
      fetchOperations.add(operation);
    }
  });

  await browser.send('Runtime.enable', {}, S);
  await browser.send('Log.enable', {}, S);
  await browser.send('Page.enable', {}, S);
  await browser.send('DOM.enable', {}, S);
  await browser.send('Network.enable', {}, S);
  const fetchPatterns = [{ urlPattern: '*', requestStage: 'Request' }];
  if (escapeProbe || localeArg) {
    fetchPatterns.push({
      urlPattern: proxy.origin + pagePath,
      requestStage: 'Response',
    });
  }
  await browser.send('Fetch.enable', { patterns: fetchPatterns }, S);
  if (!IS_SETUP) {
    await browser.send('Network.setExtraHTTPHeaders', {
      headers: { Authorization: 'Bearer root:test-password-1' },
    }, S);
  }
  await browser.send('Page.addScriptToEvaluateOnNewDocument', {
    source: `
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
    `,
  }, S);

  const loaded = new Promise((resolve) => {
    browser.on((m) => {
      if (m.sessionId === S && m.method === 'Page.loadEventFired') resolve();
    });
  });
  await browser.send('Page.navigate', {
    url: proxy.origin + (IS_SETUP ? '/setup' : '/'),
  }, S);
  await Promise.race([loaded, sleep(20000)]);

  const state = await evaluateRaw(browser, S, 'document.readyState');
  if (state !== 'complete') {
    console.error(`FAIL — page never finished loading (readyState=${state}).`);
    throw reportedFailure();
  }
  // let the first poll land and paint
  await sleep(1500);
  await Promise.all([...fetchOperations]);
  const requiredPresentation = IS_SETUP
    ? ['/setup', '/assets/public/public.css', '/assets/public/setup.js']
    : ['/', '/assets/operator/operator.css', '/assets/operator/shared.js',
       '/assets/operator/dashboard.js', '/assets/operator/settings.js'];
  for (const resource of requiredPresentation) {
    if (!requestedPresentation.has(resource)) initialAssetErrors.push(`not requested ${resource}`);
    if (!servedPresentation.has(resource)) initialAssetErrors.push(`not served ${resource}`);
  }
  if ((escapeProbe || localeArg) && !serverPageResponses.has(pagePath)) {
    initialAssetErrors.push(`probe did not mutate a server response ${pagePath}`);
  }
  if (initialAssetErrors.length) {
    console.error('FAIL — initial presentation asset load was incomplete or external');
    for (const error of new Set(initialAssetErrors)) console.error('  ' + error);
    throw reportedFailure();
  }

  if (process.env.DEBUG) {
    const diag = await browser.send('Runtime.evaluate', {
      expression: `JSON.stringify({
        fetches: ${JSON.stringify(fetched)}.slice(0, 8),
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

  /* Generated metric geometry changes on every poll. Prove the CSSOM bridge
     bounds its cache/rules without invalidating a live node during compaction. */
  if (!IS_SETUP) {
    const dynamicStyleFailure = await evaluate(`
      (() => {
        if (typeof MAX_DYNAMIC_STYLE_RULES !== 'number')
          return 'MAX_DYNAMIC_STYLE_RULES is not defined';
        const anchor = document.createElement('div');
        anchor.dataset.style = 'position:absolute;width:37px;height:11px';
        document.body.appendChild(anchor);
        applyDynamicStyles(anchor);
        const before = anchor.getBoundingClientRect();
        const compactionsBefore = dynamicStyleCompactions;
        for (let i = 0; i < MAX_DYNAMIC_STYLE_RULES * 2; i++) {
          const node = document.createElement('div');
          node.dataset.style = 'position:absolute;width:' + (1000 + i) + 'px';
          document.body.appendChild(node);
          applyDynamicStyles(node);
          node.remove();
        }
        const after = anchor.getBoundingClientRect();
        const sheet = [...document.styleSheets].find(candidate =>
          candidate.href?.endsWith('/assets/operator/operator.css'));
        const rules = [...sheet.cssRules].filter(rule =>
          rule.selectorText?.startsWith('.dynamic-style-')).length;
        const problem =
          dynamicStyleCompactions <= compactionsBefore
            ? 'probe did not exercise live-node compaction'
            : dynamicStyleClasses.size > MAX_DYNAMIC_STYLE_RULES
            ? 'dynamic style cache exceeded its bound'
            : rules > MAX_DYNAMIC_STYLE_RULES
              ? 'dynamic stylesheet exceeded its bound'
              : rules !== dynamicStyleClasses.size
                ? 'dynamic style cache and stylesheet disagree'
              : before.width !== after.width || before.height !== after.height
                ? 'compaction changed live-node geometry'
                : '';
        anchor.remove();
        return problem;
      })()`);
    if (dynamicStyleFailure) {
      console.error(`[dynamic-style-bound] FAIL — ${dynamicStyleFailure}`);
      throw reportedFailure();
    }
  }

  /* Exercise the page's real ID/descriptor runtime. Static self-tests cover
     source-level forbidden contexts; this probe proves the helpers use native
     DOM sinks, preserve literal Unicode/entities/markup, and reject every
     non-text attribute. */
  if (escapeProbe) {
    const sinkContext = await evaluate(`
      (() => {
      const required = ['setMessageText', 'setMessageAttr'];
      for (const name of required) {
        if (typeof globalThis[name] !== 'function') return name + ' is not reachable';
      }
      if (typeof message !== 'function') return 'lexical message resolver is not reachable';
      if (typeof globalThis.message !== 'undefined')
        return 'raw message resolver is exposed on globalThis';
      if (!${JSON.stringify(IS_SETUP)} &&
          (typeof catalogMessage !== 'function' || typeof escapeHtml !== 'function'))
        return 'catalog descriptor or HTML sink is not reachable';
      if (typeof I18N_TEXT_ATTRS === 'undefined') return 'I18N_TEXT_ATTRS is not defined';
      const allowedAttrs = [...I18N_TEXT_ATTRS].sort().join(',');
      if (allowedAttrs !== 'alt,aria-label,placeholder,title')
        return 'I18N_TEXT_ATTRS is not exactly the four approved attributes';
      for (const old of ['rawMsg', 't', 'tRaw', 'tHtml']) {
        if (typeof globalThis[old] !== 'undefined') return old + ' escaped/plain compatibility helper survives';
      }
      const ids = Object.keys(MSG);
      if (!ids.length) return 'catalog has no probe id';
      const id = ids[0];
      const expected = message(id);
      if (!expected.includes(${JSON.stringify(PROBE_MARKER)})) return 'message() did not return the hostile probe';
      if (!expected.includes('<b>${PROBE_TAG_TEXT}</b>')) return 'message() did not return literal markup text';

      const host = document.createElement('div');
      document.body.appendChild(host);
      const problems = [];

      if (!${JSON.stringify(IS_SETUP)}) {
        const descriptor = catalogMessage(id);
        if (!Object.isFrozen(descriptor)) problems.push('catalog descriptor is mutable');
        let coercionError = '';
        try { String(descriptor); } catch (e) { coercionError = e.message; }
        if (coercionError !== 'catalog descriptor requires escapeHtml')
          problems.push('catalog descriptor did not refuse ordinary coercion');
        const escaped = expected.replace(/[&<>"']/g, c =>
          ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
        if (escapeHtml(descriptor) !== escaped)
          problems.push('HTML sink did not resolve and escape the catalog descriptor');
        for (const attempt of [
          d => { document.createElement('a').href = d; },
          d => { document.createElement('div').style.cssText = d; },
          d => { document.createElement('div').setAttribute('title', d); },
        ]) {
          let error = '';
          try { attempt(descriptor); } catch (e) { error = e.message; }
          if (error !== 'catalog descriptor requires escapeHtml')
            problems.push('catalog descriptor did not refuse a coercive non-HTML sink');
        }
      }

      const text = document.createElement('span');
      host.appendChild(text);
      setMessageText(text, id);
      if (text.textContent !== expected) problems.push('element text changed catalog bytes');
      if (text.children.length) problems.push('element text parsed catalog markup');
      const textHost = document.createElement('span');
      const textNode = document.createTextNode('');
      textHost.appendChild(textNode);
      host.appendChild(textHost);
      setMessageText(textNode, id);
      if (textNode.textContent !== expected)
        problems.push('ordinary parented text node changed catalog bytes');

      for (const node of [
        document.createElement('script'),
        document.createElement('style'),
        document.createElementNS('http://www.w3.org/2000/svg', 'svg'),
      ]) {
        let error = '';
        try { setMessageText(node, id); }
        catch (e) { error = e.message; }
        if (error !== 'forbidden catalog text target')
          problems.push(node.nodeName + ' did not throw the stable text-target refusal');
        if (node.textContent) problems.push(node.nodeName + ' received catalog text');
      }
      for (const parent of [
        document.createElement('script'),
        document.createElement('style'),
        document.createElementNS('http://www.w3.org/2000/svg', 'svg'),
      ]) {
        const textNode = document.createTextNode('');
        parent.appendChild(textNode);
        let error = '';
        try { setMessageText(textNode, id); }
        catch (e) { error = e.message; }
        if (error !== 'forbidden catalog text target')
          problems.push(parent.nodeName + ' parented text node was accepted');
        if (textNode.textContent) problems.push(parent.nodeName + ' parent received catalog text');
      }

      for (const attr of ['title', 'placeholder', 'aria-label', 'alt']) {
        const node = document.createElement(attr === 'alt' ? 'img' : 'input');
        host.appendChild(node);
        try { setMessageAttr(node, attr, id); }
        catch (e) { problems.push('refused allowlisted attribute ' + attr + ': ' + e.message); continue; }
        if (node.getAttribute(attr) !== expected) problems.push(attr + ' changed catalog bytes');
      }
      const accessibleSvg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      setMessageAttr(accessibleSvg, 'aria-label', id);
      if (accessibleSvg.getAttribute('aria-label') !== expected)
        problems.push('SVG aria-label changed catalog bytes');

      for (const attr of ['href', 'src', 'style', 'onclick']) {
        const node = document.createElement('a');
        host.appendChild(node);
        let error = '';
        try { setMessageAttr(node, attr, id); }
        catch (e) { error = e.message; }
        if (error !== 'forbidden catalog attribute: ' + attr)
          problems.push(attr + ' did not throw the stable refusal');
        if (node.hasAttribute(attr)) problems.push(attr + ' was set from a catalog id');
      }
      if (typeof setMessageWithNodes === 'function') {
        const id = 'setup.step3.mintkey';
        const original = MSG[id].en;
        const key = document.createElement('code');
        key.textContent = 'KEY';
        try {
          MSG[id].en = 'A {key} B {key}';
          setMessageWithNodes(host, id, [['{key}', key]]);
          if (host.textContent !== 'A KEY B KEY')
            problems.push('structured replacement did not preserve repeated placeholders');
          if (host.querySelectorAll('code').length !== 2)
            problems.push('structured replacement did not create one fixed node per placeholder');
        } finally {
          MSG[id].en = original;
        }
        for (const node of [
          document.createElement('script'),
          document.createElement('style'),
          document.createElementNS('http://www.w3.org/2000/svg', 'svg'),
        ]) {
          let error = '';
          try { setMessageWithNodes(node, id, [['{key}', key]]); }
          catch (e) { error = e.message; }
          if (error !== 'forbidden catalog text target')
            problems.push(node.nodeName + ' structured helper did not refuse its target');
          if (node.textContent) problems.push(node.nodeName + ' received structured catalog text');
        }
        for (const replacement of [
          document.createElement('script'),
          document.createElement('style'),
          document.createElementNS('http://www.w3.org/2000/svg', 'svg'),
          (() => {
            const wrapper = document.createElement('span');
            wrapper.appendChild(document.createElement('script'));
            return wrapper;
          })(),
        ]) {
          let error = '';
          try { setMessageWithNodes(host, id, [['{key}', replacement]]); }
          catch (e) { error = e.message; }
          if (error !== 'forbidden catalog text target')
            problems.push(replacement.nodeName + ' structured replacement was accepted');
        }
      }
      if (typeof setEmphasizedMessage === 'function') {
        const id = 'setup.step3.mintwarn';
        for (const node of [
          document.createElement('script'),
          document.createElement('style'),
          document.createElementNS('http://www.w3.org/2000/svg', 'svg'),
        ]) {
          let error = '';
          try { setEmphasizedMessage(node, id, document.createElement('b')); }
          catch (e) { error = e.message; }
          if (error !== 'forbidden catalog text target')
            problems.push(node.nodeName + ' emphasis helper did not refuse its target');
          if (node.textContent) problems.push(node.nodeName + ' received emphasized catalog text');
        }
        for (const emphasis of [
          document.createElement('script'),
          document.createElement('style'),
          document.createElementNS('http://www.w3.org/2000/svg', 'svg'),
          (() => {
            const wrapper = document.createElement('b');
            wrapper.appendChild(document.createElement('script'));
            return wrapper;
          })(),
        ]) {
          let error = '';
          try { setEmphasizedMessage(host, id, emphasis); }
          catch (e) { error = e.message; }
          if (error !== 'forbidden catalog text target')
            problems.push(emphasis.nodeName + ' emphasis replacement was accepted');
        }
      }
      host.remove();
      return problems.length ? problems.join('; ') : '';
      })()`);
    if (sinkContext) {
      console.error(`[sink-context] FAIL — ${PAGE_REL}: ${sinkContext}`);
      throw reportedFailure();
    }
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
      throw reportedFailure();
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

  await cleanupRun();

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
    throw reportedFailure();
  }

  if (doubleEscaped.length) {
    const dbl = doubleEscaped.filter((d) => d.dir !== 'missing');
    const miss = doubleEscaped.filter((d) => d.dir === 'missing');
    console.error(`\nFAIL — contextual catalog sink violated: ${dbl.length} entity-escaped, ${miss.length} parsed as markup`);
    if (dbl.length) console.error('  a text/attribute sink rendered escaped entities:');
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
    throw reportedFailure();
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
    throw reportedFailure();
  }

  console.log('render ok — no uncaught page errors');
  } finally {
    await cleanupRun();
  }
}

if (args.includes('--cleanup-selftest')) {
  cleanupSelftest().then(code => process.exit(code)).catch((e) => {
    console.error('cleanup selftest failed to run: ' + e.message);
    process.exit(2);
  });
} else {
  main().catch((e) => {
    if (!e.reported) console.error('render_check failed to run: ' + e.message);
    process.exitCode = e.exitCode || 2;
  });
}
