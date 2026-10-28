// The deploy's workflow test (docs/11-playground.md §5.1): fails unless the assembled
// site has a `/v/<version>/` for every playground release asset, each built for its own
// path, the root serves the newest release, and `/next/` exists.
//   node scripts/check-site.mjs <site> <assets dir>
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { newest } from './release.mjs';

const ASSET = /^playground-(.+)\.tar\.gz$/;

/** The versions of the `playground-<version>.tar.gz` assets in `dir`. */
export function assetVersions(dir) {
  return fs
    .readdirSync(dir)
    .map((name) => ASSET.exec(name)?.[1])
    .filter((v) => v !== undefined);
}

function footer(html) {
  const m = /<footer\b[^>]*\bdata-version="([^"]*)"/.exec(html);
  return m?.[1];
}

/** Root-relative `src`/`href` values that do not start with the page's own base. */
function foreignUrls(html, base) {
  return [...html.matchAll(/\b(?:src|href)="(\/[^"]*)"/g)].map((m) => m[1]).filter((u) => !u.startsWith(base));
}

function checkPage(site, dir, base, version, problems) {
  const file = path.join(site, dir, 'index.html');
  if (!fs.existsSync(file)) {
    problems.push(`${dir || '/'}: no index.html`);
    return;
  }
  const html = fs.readFileSync(file, 'utf8');
  const found = footer(html);
  if (version !== undefined && found !== version) {
    problems.push(`${dir || '/'}: footer version is ${found ?? 'missing'}, expected ${version}`);
  }
  for (const url of foreignUrls(html, base)) {
    problems.push(`${dir || '/'}: ${url} is outside ${base}`);
  }
}

/** Everything wrong with the site at `site`, given the release `versions`; empty if fine. */
export function checkSite(site, versions) {
  const problems = [];
  for (const v of versions) checkPage(site, `v/${v}`, `/pemba/v/${v}/`, v, problems);
  const root = newest(versions);
  if (root === undefined) {
    // Before the first playground release the root only points at /next/.
    if (!fs.existsSync(path.join(site, 'index.html'))) problems.push('/: no index.html');
  } else {
    checkPage(site, '', '/pemba/', root, problems);
  }
  checkPage(site, 'next', '/pemba/next/', undefined, problems);
  return problems;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [site, assets] = process.argv.slice(2);
  if (!site || !assets) {
    console.error('usage: node scripts/check-site.mjs <site> <assets dir>');
    process.exit(2);
  }
  const versions = assetVersions(assets);
  const problems = checkSite(site, versions);
  for (const p of problems) console.error(p);
  if (problems.length > 0) process.exit(1);
  console.log(`site ok: root ${newest(versions) ?? '(no release yet)'}, /v/ ${versions.length} release(s), /next/`);
}
