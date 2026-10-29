import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { prepare, stableVersion } from './content.mjs';

test('stable docs never select a prerelease, including a deployment triggered by an older tag', () => {
  assert.equal(stableVersion(['0.1.0-alpha.4']), undefined);
  assert.equal(stableVersion(['0.1.0', '0.2.0-rc.1', '0.1.1', '0.1.0-alpha.4']), '0.1.1');
});

test('development pages reuse guides and rewrite repo links at the exact source revision', () => {
  const out = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-docs-'));
  try {
    prepare(fileURLToPath(new URL('../../..', import.meta.url)), out, { channel: 'next', version: '0.1.0-alpha.4', commit: 'abc123' });
    const guide = fs.readFileSync(path.join(out, 'guides/installation.md'), 'utf8');
    assert.match(guide, /\[Rust quick start\]\(\/quick-starts#rust-quick-start\)/);
    assert.match(guide, /github.com\/Joshua-Fields\/pemba\/blob\/abc123\/docs\/06-api.md/);
    assert.match(guide, /Python \| CPython 3.11/);
    assert.match(fs.readFileSync(path.join(out, 'index.md'), 'utf8'), /development build \(main\)/);
  } finally { fs.rmSync(out, { recursive: true, force: true }); }
});

test('before stable and for legacy stable tags, placeholders never claim current guides are released', () => {
  const source = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-legacy-'));
  const out = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-docs-'));
  try {
    prepare(source, out, { channel: 'stable', commit: 'abc123' });
    assert.match(fs.readFileSync(path.join(out, 'index.md'), 'utf8'), /No stable release/);
    prepare(source, out, { channel: 'stable', version: '0.1.0', commit: 'abc123' });
    assert.match(fs.readFileSync(path.join(out, 'index.md'), 'utf8'), /predates the documentation site/);
    assert.equal(fs.existsSync(path.join(out, 'guides')), false);
  } finally {
    fs.rmSync(source, { recursive: true, force: true });
    fs.rmSync(out, { recursive: true, force: true });
  }
});
