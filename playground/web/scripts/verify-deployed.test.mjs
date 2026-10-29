import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';

import { afterEach, beforeEach, expect, it } from 'vitest';

import { deployedProblems } from './verify-deployed.mjs';

let site;
let server;
let base;
let responses;

beforeEach(async () => {
  site = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-live-'));
  responses = new Map([
    ['/pemba/', Buffer.from('<footer data-version="0.1.0-alpha.4">release</footer>')],
    ['/pemba/v/0.1.0-alpha.4/', Buffer.from('versioned release')],
    ['/pemba/next/', Buffer.from('development build (main)')],
    ['/pemba/v/0.1.0-alpha.4/assets/engine.wasm', Buffer.from([0, 97, 115, 109])],
  ]);
  for (const [url, body] of responses) {
    const relative = url.slice('/pemba/'.length);
    const file = path.join(site, relative, ...(url.endsWith('/') ? ['index.html'] : []));
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, body);
  }
  server = http.createServer((request, response) => {
    const body = responses.get(new URL(request.url, base).pathname);
    response.writeHead(body === undefined ? 404 : 200);
    response.end(body);
  });
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  base = `http://127.0.0.1:${server.address().port}/pemba/`;
});

afterEach(async () => {
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  fs.rmSync(site, { recursive: true, force: true });
});

it('verifies directory URLs and binary assets against the assembled site', async () => {
  expect(await deployedProblems(site, base)).toEqual([]);
});

it('rejects a successful deployment still serving the old root redirect', async () => {
  responses.set('/pemba/', Buffer.from('<meta http-equiv="refresh" content="0; url=next/">'));
  expect(await deployedProblems(site, base)).toEqual(['/: served content differs from the assembled site']);
});

it('rejects the missing versioned directory even when root and next load', async () => {
  responses.delete('/pemba/v/0.1.0-alpha.4/');
  expect(await deployedProblems(site, base)).toEqual(['v/0.1.0-alpha.4/: HTTP 404']);
});

it('rejects HTML served instead of the release WASM binary', async () => {
  responses.set('/pemba/v/0.1.0-alpha.4/assets/engine.wasm', Buffer.from('<html>fallback</html>'));
  expect(await deployedProblems(site, base)).toEqual([
    'v/0.1.0-alpha.4/assets/engine.wasm: served content differs from the assembled site',
  ]);
});
