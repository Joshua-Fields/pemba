// Reuse the playground's existing Chromium dependency and installed browser.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(new URL('../../../playground/web/package.json', import.meta.url));
const { chromium, expect } = require('@playwright/test');
const root = fileURLToPath(new URL('../.vitepress/', import.meta.url));
const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.woff2': 'font/woff2' };
const server = http.createServer((req, res) => {
  const url = new URL(req.url, 'http://localhost');
  const next = url.pathname.startsWith('/pemba/docs/next/');
  const base = next ? '/pemba/docs/next/' : '/pemba/docs/';
  if (!url.pathname.startsWith(base)) { res.writeHead(404).end(); return; }
  let file = path.resolve(root, next ? 'dist' : 'stable', url.pathname.slice(base.length) || 'index.html');
  if (!file.startsWith(root)) { res.writeHead(403).end(); return; }
  if (!path.extname(file)) file += '.html';
  if (!fs.existsSync(file) || !fs.statSync(file).isFile()) { res.writeHead(404).end(); return; }
  res.setHeader('Content-Type', types[path.extname(file)] ?? 'application/octet-stream');
  fs.createReadStream(file).pipe(res);
});
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch();
try {
  for (const viewport of [{ width: 1280, height: 900 }, { width: 390, height: 844 }]) {
    const page = await browser.newPage({ viewport });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('response', response => { if (response.status() >= 400) errors.push(`${response.status()} ${response.url()}`); });
    await page.goto(`${origin}/pemba/docs/next/`);
    await page.waitForFunction(() => document.querySelector('.VPNavBarSearch'));
    await expect(page.getByRole('note')).toBeVisible();
    await expect(page.getByRole('note')).toContainText('development build (main)');
    const skip = page.getByRole('link', { name: 'Skip to content' });
    await page.keyboard.press('Tab');
    assert.equal(await skip.evaluate(el => el === document.activeElement), true);
    await page.keyboard.press('Enter');
    assert.match(page.url(), /#VPContent$/);
    if (viewport.width < 600) await page.getByRole('button', { name: 'Menu', exact: true }).click();
    const tutorial = page.locator('.VPSidebar').getByRole('link', { name: 'Transactional tutorial', exact: true });
    await tutorial.focus();
    await page.keyboard.press('Enter');
    await page.waitForURL('**/guides/transactional-tutorial.html');
    await expect(page.getByRole('heading', { level: 1 })).toContainText('Commit app data and permissions together');
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
    assert.match(await page.locator('.VPSidebar').innerText(), /development build \(main\)/);
    if (viewport.width < 600) await page.getByRole('button', { name: 'Menu', exact: true }).click();
    const contributing = page.locator('.VPSidebar').getByRole('link', { name: 'Contributing', exact: true });
    await contributing.focus();
    await page.keyboard.press('Enter');
    await page.waitForURL('**/contributing.html');
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Contributing to pemba');
    if (viewport.width < 600) await page.getByRole('button', { name: 'Menu', exact: true }).click();
    const security = page.locator('.VPSidebar').getByRole('link', { name: 'Security', exact: true });
    await security.focus();
    await page.keyboard.press('Enter');
    await page.waitForURL('**/security.html');
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Security policy');
    await expect(page.getByRole('link', { name: 'GitHub private vulnerability reporting', exact: true }))
      .toHaveAttribute('href', 'https://github.com/Joshua-Fields/pemba/security/advisories/new');
    await expect(page.getByRole('note')).toContainText('development build (main)');
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true);
    await page.screenshot({ path: path.join(root, `docs-${viewport.width}.png`), fullPage: false });
    await page.goto(`${origin}/pemba/docs/`);
    assert.match(await page.getByRole('heading', { level: 1 }).innerText(), /No stable release/);
    assert.deepEqual(errors, []);
    await page.close();
  }
  console.log('docs browser: desktop/mobile, keyboard navigation, labels and assets passed');
} finally {
  await browser.close();
  server.close();
}
