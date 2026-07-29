#!/usr/bin/env node
// Golden-output harness for the dashboard's number/duration/date formatters.
//
//   TZ=UTC LC_ALL=en_US.UTF-8 node scripts/formatter_fixture.js
//   TZ=UTC LC_ALL=en_US.UTF-8 node scripts/formatter_fixture.js --check
//
// Captured BEFORE the Intl migration so the migration's diff *is* the review
// evidence: every changed line has to be justified in the PR, and every
// unchanged line proves the migration did not move something it shouldn't.
//
// TZ and locale are pinned because these functions read the system default
// (`toLocaleString([])`). Without pinning, the fixture would encode whichever
// machine last ran it.

const fs = require("fs");
const path = require("path");

if (process.env.TZ !== "UTC") {
  console.error("refusing to run: set TZ=UTC so the fixture is reproducible");
  process.exit(2);
}

const ROOT = path.join(__dirname, "..");
const src = fs.readFileSync(path.join(ROOT, "src/dashboard.html"), "utf8");

// Pull the formatter definitions straight out of the page so the fixture can
// never drift from the code it is meant to pin.
function grab(re, what) {
  const m = src.match(re);
  if (!m) {
    console.error(`could not find ${what} in src/dashboard.html`);
    process.exit(2);
  }
  return m[0];
}

const block = grab(
  /\/\* -+ formatting -+ \*\/[\s\S]*?(?=\n\/\* -+)/,
  "the formatting block"
);
const scopeDate = grab(/const scopeDate = [^\n]*\n/, "scopeDate");
const scopeTime = grab(/const scopeTime = seconds =>[\s\S]*?\}\);\n/, "scopeTime");
const axis = `
const axisLabel = (t, spanSecs) => spanSecs > 86400 * 2
  ? new Date(t).toLocaleDateString([], {month:'short', day:'numeric'})
  : new Date(t).toLocaleTimeString([], {hour:'2-digit', minute:'2-digit'});
const stamp = t => new Date(t).toLocaleString();
`;

const F = {};
new Function(
  `${block}\n${scopeDate}\n${scopeTime}\n${axis}\n` +
    `Object.assign(this, { fmt, secs, ago, scopeDate, scopeTime, axisLabel, stamp });`
).call(F);

// Fixed vectors. Chosen to sit on every branch boundary in each function —
// a migration that changes a threshold shows up here rather than in production.
const NUMBERS = [
  NaN, Infinity, -Infinity, 0, 0.004, 0.05, 0.5, 1, 1.5, 9.99, 10, 10.04, 99.9,
  100, 999, 1000, 9999, 1e4, 12345, 999999, 1e6, 1234567, 1e9, 1234567890,
  -1, -1234, -1e6, 1e12,
];
const DURATIONS = [
  NaN, 0, 0.0004, 0.001, 0.5, 0.999, 1, 1.04, 59.9, 89.9, 90, 3599, 3600, 86399,
];
const AGES = [0, 1, 59, 60, 61, 3599, 3600, 3661, 86399, 86400, 90061, 8640000];
// fixed instants, UTC
const TIMES = [
  0, 1e9, 1751328000, 1751328000 + 3600, 1751328000 + 86400 * 45,
  1767225600, 1767225600 + 45296,
];

const lines = [];
const row = (fn, input, out) => lines.push(`${fn}\t${String(input)}\t${out}`);

for (const n of NUMBERS) row("fmt", n, F.fmt(n));
for (const s of DURATIONS) row("secs", s, F.secs(s));
for (const s of AGES) row("ago", s, F.ago(s));
for (const t of TIMES) row("scopeDate", t, F.scopeDate(t));
for (const t of TIMES) row("scopeTime", t, F.scopeTime(t));
for (const t of TIMES) row("axisLabel.short", t, F.axisLabel(t * 1000, 3600));
for (const t of TIMES) row("axisLabel.long", t, F.axisLabel(t * 1000, 86400 * 30));
for (const t of TIMES) row("stamp", t, F.stamp(t * 1000));
// percentages and fixed-precision metrics go through toFixed at 39 sites
for (const v of [0, 0.5, 1, 9.94, 9.96, 10, 99.99, 100]) {
  row("pct1", v, v.toFixed(1) + "%");
  row("pct0", v, v.toFixed(0) + "%");
}

const out = lines.join("\n") + "\n";
const golden = path.join(ROOT, "tests/fixtures/formatters-en-US.txt");

if (process.argv.includes("--check")) {
  if (!fs.existsSync(golden)) {
    console.error("no golden fixture; run without --check to write one");
    process.exit(2);
  }
  const want = fs.readFileSync(golden, "utf8");
  if (want === out) {
    console.log(`formatters unchanged — ${lines.length} cases`);
    process.exit(0);
  }
  const a = want.split("\n"),
    b = out.split("\n");
  console.error("formatter output changed:");
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    if (a[i] !== b[i]) console.error(`  - ${a[i] ?? "(none)"}\n  + ${b[i] ?? "(none)"}`);
  }
  process.exit(1);
}

fs.mkdirSync(path.dirname(golden), { recursive: true });
fs.writeFileSync(golden, out);
console.log(`wrote ${lines.length} cases to ${path.relative(ROOT, golden)}`);
