import { execSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { defineConfig } from 'vitest/config';

const root = fileURLToPath(new URL('../..', import.meta.url));

// `PLAYGROUND_BASE` is the path the site is served under (docs/11-playground.md §5.1):
// `/pemba/`, `/pemba/v/<version>/`, `/pemba/next/`. Locally, the root.
const base = process.env['PLAYGROUND_BASE'] ?? '/';

/** The workspace version: the playground is built from the same commit as the engine. */
function workspaceVersion(): string {
  const toml = readFileSync(`${root}Cargo.toml`, 'utf8');
  const section = toml.slice(toml.indexOf('[workspace.package]'));
  const m = /^version\s*=\s*"([^"]+)"/m.exec(section);
  if (!m?.[1]) throw new Error('no [workspace.package] version in Cargo.toml');
  return m[1];
}

function shortCommit(): string {
  try {
    return execSync('git rev-parse --short HEAD', { cwd: root, encoding: 'utf8' }).trim();
  } catch {
    return 'unknown';
  }
}

const version = workspaceVersion();
// A link names the engine that produced its answers (§4): on /next/ a release number
// alone would not, so the commit goes with it.
const engine = base.endsWith('/next/') ? `${version}+${shortCommit()}` : version;

export default defineConfig({
  base,
  define: { __PEMBA_ENGINE__: JSON.stringify(engine) },
  // No SPA fallback: a missing `/v/<version>/` must answer 404, as GitHub Pages does,
  // so the version banner's probe is not fooled locally.
  appType: 'mpa',
  // The examples are `testdata/fixtures` files, outside this package.
  server: { fs: { allow: [root] } },
  build: { target: 'es2022' },
  worker: { format: 'es' },
  test: { include: ['src/**/*.test.ts'] },
});
