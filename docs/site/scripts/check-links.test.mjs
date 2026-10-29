import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { linkProblems } from './check-links.mjs';

test('built links check project bases, fragments and assets without an SPA fallback', () => {
  const out = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-links-'));
  try {
    fs.writeFileSync(path.join(out, 'index.html'), '<a href="/pemba/docs/next/guide.html#commit">Commit</a><script src="/pemba/docs/next/app.js"></script>');
    fs.writeFileSync(path.join(out, 'guide.html'), '<h1 id="commit">Commit</h1>');
    fs.writeFileSync(path.join(out, 'app.js'), '');
    assert.deepEqual(linkProblems(out, '/pemba/docs/next/'), []);
    fs.writeFileSync(path.join(out, 'guide.html'), '<a href="/pemba/docs/next/missing.html">bad</a>');
    fs.unlinkSync(path.join(out, 'app.js'));
    const problems = linkProblems(out, '/pemba/docs/next/');
    assert.ok(problems.some(p => p.includes('missing.html')));
    assert.ok(problems.some(p => p.includes('#commit')));
    assert.ok(problems.some(p => p.includes('app.js')));
  } finally { fs.rmSync(out, { recursive: true, force: true }); }
});
