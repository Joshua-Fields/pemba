// Pages can report success while still serving an earlier artifact. Compare the live
// directory URLs and every asset with the already-validated assembled site.
import fs from 'node:fs';
import path from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

function files(dir, prefix = '') {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const relative = prefix + entry.name;
    return entry.isDirectory() ? files(path.join(dir, entry.name), `${relative}/`) : [relative];
  });
}

/** Problems with the served bytes, not just the deployment API's status. */
export async function deployedProblems(site, pageUrl) {
  const base = pageUrl.endsWith('/') ? pageUrl : `${pageUrl}/`;
  const manifest = files(site).sort();
  if (manifest.length === 0) throw new Error('assembled site is empty');
  const problems = [];
  const nonce = Date.now().toString();
  for (let i = 0; i < manifest.length; i += 4) {
    const batch = await Promise.all(manifest.slice(i, i + 4).map(async (relative) => {
      const requestPath = relative === 'index.html' || relative.endsWith('/index.html')
        ? relative.slice(0, -'index.html'.length) : relative;
      const label = requestPath || '/';
      const url = new URL(requestPath, base);
      url.searchParams.set('pemba-verify', nonce);
      try {
        const response = await fetch(url, {
          headers: { 'Cache-Control': 'no-cache' },
          signal: AbortSignal.timeout(10_000),
        });
        if (response.status !== 200) return `${label}: HTTP ${response.status}`;
        const actual = Buffer.from(await response.arrayBuffer());
        if (!fs.readFileSync(path.join(site, relative)).equals(actual)) {
          return `${label}: served content differs from the assembled site`;
        }
        return undefined;
      } catch (error) {
        return `${label}: ${error.message}`;
      }
    }));
    problems.push(...batch.filter((problem) => problem !== undefined));
  }
  return problems;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [site, pageUrl] = process.argv.slice(2);
  if (!site || !pageUrl) {
    console.error('usage: node scripts/verify-deployed.mjs <assembled site> <page URL>');
    process.exit(2);
  }
  // Allow the CDN to converge, but never mark stale or missing content as verified.
  for (let attempt = 1; attempt <= 24; attempt++) {
    const problems = await deployedProblems(site, pageUrl);
    if (problems.length === 0) {
      console.log('live site matches the assembled site: all pages and assets');
      process.exit(0);
    }
    console.error(`live verification ${attempt}/24: ${problems.join('; ')}`);
    if (attempt < 24) await sleep(10_000);
  }
  process.exit(1);
}
