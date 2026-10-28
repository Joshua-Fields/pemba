import { execSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import type { Plugin } from 'vite';
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

function git(...args: string[]): string {
  try {
    return execSync(['git', ...args].join(' '), { cwd: root, encoding: 'utf8' }).trim();
  } catch {
    return 'unknown';
  }
}

const version = workspaceVersion();
const commit = git('rev-parse', 'HEAD');
const shortCommit = git('rev-parse', '--short', 'HEAD');
// A link names the engine that produced its answers (§4): on /next/ a release number
// alone would not, so the commit goes with it.
const engine = base.endsWith('/next/') ? `${version}+${shortCommit}` : version;

/**
 * Writes the version and commit into the page footer (§5) as static HTML, so the deploy
 * check (`scripts/check-site.mjs`) can read them without running the page.
 */
function buildInfo(): Plugin {
  const values: Record<string, string> = {
    PEMBA_VERSION: version,
    PEMBA_COMMIT: commit,
    PEMBA_SHORT_COMMIT: shortCommit,
  };
  return {
    name: 'pemba-build-info',
    transformIndexHtml: {
      order: 'pre',
      handler: (html) => html.replace(/%(PEMBA_[A-Z_]+)%/g, (all, key: string) => values[key] ?? all),
    },
  };
}

export default defineConfig({
  base,
  plugins: [buildInfo()],
  define: { __PEMBA_ENGINE__: JSON.stringify(engine) },
  // No SPA fallback: a missing `/v/<version>/` must answer 404, as GitHub Pages does,
  // so the version banner's probe is not fooled locally.
  appType: 'mpa',
  // The examples are `testdata/fixtures` files, outside this package.
  server: { fs: { allow: [root] } },
  build: { target: 'es2022' },
  worker: { format: 'es' },
  test: { include: ['src/**/*.test.ts', 'scripts/**/*.test.mjs'] },
});
