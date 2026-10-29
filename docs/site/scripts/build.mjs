import fs from 'node:fs';
import path from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { prepare } from './content.mjs';
import { linkProblems } from './check-links.mjs';

const root = fileURLToPath(new URL('..', import.meta.url));
const args = process.argv.slice(2);
function option(name, fallback) {
  const index = args.indexOf(`--${name}`);
  return index < 0 ? fallback : args[index + 1];
}
const channel = option('channel', 'next');
if (!['stable', 'next'].includes(channel)) throw new Error('channel must be stable or next');
const source = path.resolve(option('source', path.join(root, '../..')));
const out = path.resolve(option('out', path.join(root, '.vitepress/dist')));
const base = channel === 'next' ? '/pemba/docs/next/' : '/pemba/docs/';
const commit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: source, encoding: 'utf8' }).trim();
const toml = fs.readFileSync(path.join(source, 'Cargo.toml'), 'utf8').split('[workspace.package]')[1];
const version = channel === 'stable' && option('empty', 'false') === 'true'
  ? undefined : /^version\s*=\s*"([^"]+)"/m.exec(toml)?.[1];
if (channel === 'stable' && version?.includes('-')) throw new Error('stable docs require a stable source version');
const generated = path.join(root, 'generated');
// Only this fixed ignored directory is removed, never the supplied source/output.
fs.rmSync(generated, { recursive: true, force: true });
prepare(source, generated, { channel, version, commit });
const cli = path.join(root, 'node_modules/vitepress/bin/vitepress.js');
const result = spawnSync(process.execPath, [cli, 'build', root, '--outDir', out], {
  env: { ...process.env, DOCS_BASE: base }, encoding: 'utf8',
});
process.stdout.write(result.stdout ?? '');
process.stderr.write(result.stderr ?? '');
if (result.error) throw result.error;
if (result.status !== 0 || /\bwarn(?:ing)?\b|falling back|\(!\)/i.test(`${result.stdout}\n${result.stderr}`)) {
  throw new Error(`documentation build failed or warned (exit ${result.status})`);
}
const problems = linkProblems(out, base);
if (problems.length) throw new Error(problems.join('\n'));
fs.copyFileSync(path.join(generated, 'build-info.json'), path.join(out, 'docs-build.json'));
console.log(`docs ok: ${base} — ${version ?? 'no stable release'}`);
