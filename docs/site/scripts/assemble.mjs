// Run by the existing Pages deployer AFTER all playground copies are assembled.
// Always run main's builder, even when the selected stable source predates it.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { assetVersions } from '../../../playground/web/scripts/check-site.mjs';
import { stableVersion } from './content.mjs';

const defaultRoot = fileURLToPath(new URL('../../..', import.meta.url));

/** Assemble docs using main's builder and stable source selected from all releases. */
export function assemble(site, assets, repository = defaultRoot) {
  const root = repository;
  const version = stableVersion(assetVersions(assets));
  const build = fileURLToPath(new URL('build.mjs', import.meta.url));
  const stable = ['--channel', 'stable', '--out', path.resolve(site, 'docs')];
  if (version) {
    const temporary = fs.mkdtempSync(path.join(os.tmpdir(), 'pemba-stable-docs-'));
    const checkout = path.join(temporary, 'source');
    try {
      execFileSync('git', ['worktree', 'add', '--detach', checkout, `v${version}`], { cwd: root, stdio: 'inherit' });
      execFileSync(process.execPath, [build, ...stable, '--source', checkout], { stdio: 'inherit' });
    } finally {
      // Both paths are fixed children of the freshly created temporary directory.
      if (fs.existsSync(checkout)) execFileSync('git', ['worktree', 'remove', '--force', checkout], { cwd: root });
      fs.rmSync(temporary, { recursive: true, force: true });
    }
  } else {
    execFileSync(process.execPath, [build, ...stable, '--empty', 'true'], { stdio: 'inherit' });
  }
  // Build stable first: its output directory is the parent of development's.
  execFileSync(process.execPath, [build, '--channel', 'next', '--out', path.resolve(site, 'docs/next')], { stdio: 'inherit' });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [site, assets] = process.argv.slice(2);
  if (!site || !assets) throw new Error('usage: node scripts/assemble.mjs <site> <release-assets>');
  assemble(site, assets);
}
