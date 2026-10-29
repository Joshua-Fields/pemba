import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { test } from 'node:test';
import { assemble } from './assemble.mjs';
import { checkSite } from '../../../playground/web/scripts/check-site.mjs';

test('an older tag deploy preserves playground bytes and serves newest stable docs plus main docs', () => {
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-assembly-'));
  const source = path.join(temp, 'repository');
  const site = path.join(temp, 'site');
  const assets = path.join(temp, 'assets');
  fs.mkdirSync(source); fs.mkdirSync(assets); fs.mkdirSync(site);
  const git = (...args) => execFileSync('git', args, { cwd: source, stdio: 'pipe' });
  try {
    git('init'); git('config', 'user.email', 'test@example.invalid'); git('config', 'user.name', 'Test');
    for (const version of ['0.1.0', '0.1.1', '0.2.0-rc.1']) {
      fs.writeFileSync(path.join(source, 'Cargo.toml'), `[workspace.package]\nversion = "${version}"\n`);
      git('add', '.'); git('commit', '-m', version); git('tag', `v${version}`);
      fs.writeFileSync(path.join(assets, `playground-${version}.tar.gz`), 'asset presence selects release');
      const dir = path.join(site, 'v', version);
      fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(path.join(dir, 'index.html'), `<footer data-version="${version}"></footer>`);
      fs.writeFileSync(path.join(dir, 'engine.wasm'), `immutable ${version}`);
    }
    // Simulate a dispatch caused by the older v0.1.0 tag. The complete release
    // inventory, not that triggering tag or a newer prerelease, selects docs.
    git('checkout', 'v0.1.0');
    fs.writeFileSync(path.join(site, 'index.html'), '<footer data-version="0.1.1"></footer>');
    fs.mkdirSync(path.join(site, 'next'));
    fs.writeFileSync(path.join(site, 'next/index.html'), 'development build (main)<footer data-version="0.2.0-rc.1"></footer>');
    const immutable = fs.readFileSync(path.join(site, 'v/0.1.0/engine.wasm'));
    const root = fs.readFileSync(path.join(site, 'index.html'));
    assemble(site, assets, source);
    assert.deepEqual(fs.readFileSync(path.join(site, 'v/0.1.0/engine.wasm')), immutable);
    assert.deepEqual(fs.readFileSync(path.join(site, 'index.html')), root);
    const info = JSON.parse(fs.readFileSync(path.join(site, 'docs/docs-build.json')));
    assert.equal(info.version, '0.1.1');
    assert.equal(info.commit, git('rev-parse', 'v0.1.1').toString().trim());
    assert.match(fs.readFileSync(path.join(site, 'docs/index.html'), 'utf8'), /predates the documentation site/);
    assert.ok(fs.existsSync(path.join(site, 'docs/next/guides/transactional-tutorial.html')));
    assert.deepEqual(checkSite(site, ['0.1.0', '0.1.1', '0.2.0-rc.1'], { docs: true }), []);
    fs.writeFileSync(path.join(site, 'docs/docs-build.json'), JSON.stringify({ channel: 'stable', version: '0.2.0-rc.1' }));
    assert.ok(checkSite(site, ['0.1.0', '0.1.1', '0.2.0-rc.1'], { docs: true }).includes('docs: wrong channel/version'));
  } finally { fs.rmSync(temp, { recursive: true, force: true }); }
});
