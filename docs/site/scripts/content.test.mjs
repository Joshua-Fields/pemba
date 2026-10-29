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

test('stable quick starts route absolute main guide links locally and pin other repo material', () => {
  const out = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-stable-links-'));
  try {
    prepare(fileURLToPath(new URL('../../..', import.meta.url)), out, { channel: 'stable', version: '0.1.0', commit: 'released123' });
    const readme = fs.readFileSync(path.join(out, 'quick-starts.md'), 'utf8');
    assert.doesNotMatch(readme, /github\.com\/Joshua-Fields\/pemba\/(?:blob|tree)\/main\//);
    assert.match(readme, /\]\(\/guides\/installation\)/);
    assert.match(readme, /github.com\/Joshua-Fields\/pemba\/blob\/released123\/crates\/pemba-difftest\/KNOWN_DIFFERENCES.md/);
  } finally { fs.rmSync(out, { recursive: true, force: true }); }
});

test('contributors follow the selected revision, including older sources without the guide', () => {
  const source = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-contributors-'));
  const out = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-docs-'));
  try {
    fs.mkdirSync(path.join(source, 'docs/site'), { recursive: true });
    fs.writeFileSync(path.join(source, 'docs/site/package.json'), '{}');
    fs.mkdirSync(path.join(source, 'docs/guides'));
    for (const name of ['installation', 'schema', 'transactions', 'transactional-tutorial', 'consistency', 'troubleshooting']) {
      fs.writeFileSync(path.join(source, `docs/guides/${name}.md`), `# ${name}\n`);
    }
    fs.writeFileSync(path.join(source, 'README.md'), '# Selected revision\n');
    let info = prepare(source, out, { channel: 'stable', version: '0.1.0', commit: 'older123' });
    assert.equal(info.contributing, false);
    assert.equal(fs.existsSync(path.join(out, 'contributing.md')), false);
    fs.writeFileSync(path.join(source, 'CONTRIBUTING.md'), '# Contributing to pemba\n\nRead [the rules](CLAUDE.md) and [installation](docs/guides/installation.md).\n');
    fs.writeFileSync(path.join(source, 'CLAUDE.md'), '# Rules\n');
    info = prepare(source, out, { channel: 'next', version: '0.1.0-alpha.4', commit: 'newer123' });
    assert.equal(info.contributing, true);
    const guide = fs.readFileSync(path.join(out, 'contributing.md'), 'utf8');
    assert.match(guide, /blob\/newer123\/CLAUDE.md/);
    assert.match(guide, /\[installation\]\(\/guides\/installation\)/);
  } finally {
    fs.rmSync(source, { recursive: true, force: true });
    fs.rmSync(out, { recursive: true, force: true });
  }
});

test('security policy follows the selected revision and is absent from older sources', () => {
  const source = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-security-'));
  const out = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-docs-'));
  try {
    fs.mkdirSync(path.join(source, 'docs/site'), { recursive: true });
    fs.writeFileSync(path.join(source, 'docs/site/package.json'), '{}');
    fs.mkdirSync(path.join(source, 'docs/guides'));
    for (const name of ['installation', 'schema', 'transactions', 'transactional-tutorial', 'consistency', 'troubleshooting']) {
      fs.writeFileSync(path.join(source, `docs/guides/${name}.md`), `# ${name}\n`);
    }
    fs.writeFileSync(path.join(source, 'README.md'), '# Selected revision\n');
    let info = prepare(source, out, { channel: 'stable', version: '0.1.0', commit: 'older123' });
    assert.equal(info.security, false);
    assert.equal(fs.existsSync(path.join(out, 'security.md')), false);
    fs.writeFileSync(path.join(source, 'SECURITY.md'), '# Security policy\n\nUse [private reporting](https://github.com/Joshua-Fields/pemba/security/advisories/new) and read [the process](docs/RELEASE-0.1.0.md).\n');
    fs.writeFileSync(path.join(source, 'docs/RELEASE-0.1.0.md'), '# Release evidence\n');
    for (const channel of ['next', 'stable']) {
      info = prepare(source, out, { channel, version: '0.1.0', commit: 'newer123' });
      assert.equal(info.security, true);
      const policy = fs.readFileSync(path.join(out, 'security.md'), 'utf8');
      assert.match(policy, /https:\/\/github.com\/Joshua-Fields\/pemba\/security\/advisories\/new/);
      assert.match(policy, /blob\/newer123\/docs\/RELEASE-0.1.0.md/);
    }
  } finally {
    fs.rmSync(source, { recursive: true, force: true });
    fs.rmSync(out, { recursive: true, force: true });
  }
});
