// Serves web/ and testdata/fixtures, loads the WASM build in headless Chromium,
// runs every fixture and times checks. Usage: node web/run.mjs
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { chromium } from 'playwright';

const root = path.resolve('web');
const fixtures = path.resolve('../../testdata/fixtures');
const types = { '.js': 'text/javascript', '.wasm': 'application/wasm', '.html': 'text/html', '.yaml': 'text/plain' };
const server = http.createServer((req, res) => {
  const url = decodeURIComponent(req.url.split('?')[0]);
  const file = url.startsWith('/fixtures/') ? path.join(fixtures, url.slice(10)) : path.join(root, url === '/' ? 'index.html' : url);
  fs.readFile(file, (e, data) => {
    if (e) { res.writeHead(404); return res.end(); }
    res.writeHead(200, { 'content-type': types[path.extname(file)] ?? 'application/octet-stream' });
    res.end(data);
  });
}).listen(0);
const port = server.address().port;
const names = fs.readdirSync(fixtures).filter(f => f.endsWith('.yaml'));

const browser = await chromium.launch();
const page = await browser.newPage();
page.on('console', m => console.log('[page]', m.text()));
page.on('pageerror', e => console.log('[pageerror]', e.message));
await page.goto(`http://localhost:${port}/`);
const result = await page.evaluate(async (names) => {
  const t0 = performance.now();
  const m = await import('/pkg/pemba_playground_spike.js');
  await m.default();
  const init = performance.now() - t0;
  const out = { init_ms: init, fixtures: {}, timings: {} };
  for (const n of names) {
    const y = await (await fetch('/fixtures/' + n)).text();
    try { out.fixtures[n] = JSON.parse(m.run_fixture(y)); } catch (e) { out.fixtures[n] = { panic: String(e) }; }
  }
  const y = await (await fetch('/fixtures/google_docs.yaml')).text();
  const pg = new m.Playground();
  // Pull schema and relationships out of the fixture with a crude split (spike only).
  const schema = y.split('schema: |-\n')[1].split('\nrelationships:')[0].replace(/^  /gm, '');
  const rels = y.split('relationships: |-\n')[1].split(/\n\S/)[0].replace(/^  /gm, '');
  out.schema = JSON.parse(pg.write_schema(schema));
  out.write = JSON.parse(pg.write(rels));
  out.bad_schema = JSON.parse(new m.Playground().write_schema('definition user {}\ndefinition doc { relation viewer: usr }'));
  const queries = Array.from(new Set([...y.matchAll(/^\s*- (\S+#\S+@\S+)\s*$/gm)].map(x => x[1])));
  out.queries = queries.length;
  try {
    out.sample = queries.slice(0, 3).map(q => [q, JSON.parse(pg.check(q))]);
    out.trace_head = pg.check_with_trace(queries[0]).slice(0, 300);
    out.explain = pg.explain(queries[0]);
    out.lookup_resources = JSON.parse(pg.lookup_resources('document', 'view', 'user:alice'));
    out.lookup_subjects = JSON.parse(pg.lookup_subjects(queries[0].split('#')[0], 'view', 'user'));
    for (const q of queries) pg.check(q); // warm
    const N = 200;
    const per = [];
    for (const q of queries) {
      const s = performance.now();
      for (let i = 0; i < N; i++) pg.check(q);
      per.push((performance.now() - s) / N * 1000);
    }
    per.sort((a, b) => a - b);
    out.timings.check_us = { min: per[0], p50: per[per.length >> 1], max: per[per.length - 1] };
    const tq = queries[0];
    let s = performance.now();
    for (let i = 0; i < N; i++) pg.check_with_trace(tq);
    out.timings.trace_us = (performance.now() - s) / N * 1000;
    s = performance.now();
    for (let i = 0; i < N; i++) pg.lookup_resources('document', 'view', 'user:alice');
    out.timings.lookup_resources_us = (performance.now() - s) / N * 1000;
    s = performance.now();
    m.run_fixture(y);
    out.timings.google_docs_fixture_ms = performance.now() - s;
  } catch (e) { out.panic = String(e); }
  return out;
}, names);
console.log(JSON.stringify(result, null, 1));
await browser.close();
server.close();
