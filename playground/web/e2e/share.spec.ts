import { readFileSync } from 'node:fs';
import { deflateRawSync } from 'node:zlib';

import { expect, test, type Page } from '@playwright/test';

import { setText } from './editor';

// T6.8 (docs/11-playground.md §4): examples, shareable links with the version banner, and
// fixture YAML import/export through the page.

const VERSION = /^version\s*=\s*"([^"]+)"/m.exec(
  readFileSync(new URL('../../../Cargo.toml', import.meta.url), 'utf8').split('[workspace.package]')[1] ?? '',
)?.[1];

const SCHEMA = 'definition user {}\ndefinition doc {\n    relation viewer: user\n    permission view = viewer\n}';

/** A link's fragment, made as the page makes one. */
function fragment(state: Record<string, unknown>): string {
  return '#s=' + deflateRawSync(Buffer.from(JSON.stringify(state))).toString('base64url');
}

function state(engine: string): Record<string, unknown> {
  return {
    v: 1,
    engine,
    schema: SCHEMA,
    relationships: 'doc:shared#viewer@user:zed',
    assertions: 'assertions:\n  assertTrue:\n    - doc:shared#view@user:zed\n',
    query: { kind: 'lookup_resources', text: 'doc#view@user:zed' },
  };
}

const editorText = (page: Page, id: string) => page.locator(`#${id} .cm-content`);

test('a shared link restores the whole state, with no banner for the same version', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#result .verdict')).toHaveText('allowed');
  await setText(page, 'schema', SCHEMA);
  await setText(page, 'relationships', 'doc:mine#viewer@user:amy');
  await setText(page, 'assertions', 'assertions:\n  assertFalse:\n    - doc:mine#view@user:bob\n');
  await page.locator('#kind').selectOption('lookup_subjects');
  await page.locator('#text').fill('doc:mine#view@user');
  await page.getByRole('button', { name: 'Run' }).click();
  await expect(page.locator('#result .ids li')).toHaveText(['amy']);

  await page.getByRole('button', { name: 'Share link' }).click();
  await expect(page.locator('#tool-status')).toHaveText(/Link copied|address bar/);
  const url = page.url();
  expect(url).toMatch(/#s=[A-Za-z0-9_-]+$/);

  const other = await page.context().newPage();
  await other.goto(url);
  await expect(other.locator('#result .ids li')).toHaveText(['amy']);
  await expect(other.locator('#kind')).toHaveValue('lookup_subjects');
  await expect(other.locator('#text')).toHaveValue('doc:mine#view@user');
  await expect(editorText(other, 'relationships')).toHaveText('doc:mine#viewer@user:amy');
  await expect(other.locator('#assertions-result .verdict')).toHaveText('1 check, all passed');
  await expect(other.locator('#version-banner')).toBeHidden();
  await expect(other.locator('#notice')).toBeHidden();
});

test('a malformed link shows an error and loads nothing from it', async ({ page }) => {
  // Valid base64url, but not deflate-raw: the state is damaged.
  await page.goto('/#s=AAAAAAAAAAAA');
  await expect(page.locator('#notice')).toBeVisible();
  await expect(page.locator('#notice')).toContainText('This link could not be read');
  await expect(page.locator('#notice')).toContainText('Nothing was loaded from it');
  // The page's own starting state, untouched.
  await expect(page.locator('#text')).toHaveValue('document:readme#view@user:anne');
  await expect(page.locator('#result .verdict')).toHaveText('allowed');
  await expect(page.locator('#version-banner')).toBeHidden();
});

test('a link missing a part loads nothing, not the parts it has', async ({ page }) => {
  const partial = state(VERSION ?? '');
  delete partial['assertions'];
  await page.goto('/' + fragment(partial));
  await expect(page.locator('#notice')).toContainText('could not be read');
  await expect(editorText(page, 'relationships')).not.toContainText('doc:shared');
  await expect(page.locator('#text')).toHaveValue('document:readme#view@user:anne');
});

test('a link from a published release links that release, without switching to it', async ({ page }) => {
  let probed = false;
  await page.route('**/v/0.0.9/', (route) => {
    probed = true;
    return route.fulfill({ status: 200, contentType: 'text/html', body: '<!doctype html>' });
  });
  const hash = fragment(state('0.0.9'));
  await page.goto('/' + hash);
  const banner = page.locator('#version-banner');
  await expect(banner).toBeVisible();
  await expect(banner).toContainText(`This link was made with pemba 0.0.9; this page runs pemba ${VERSION}.`);
  await expect(banner.getByRole('link')).toHaveAttribute('href', `/v/0.0.9/${hash}`);
  expect(probed).toBe(true);
  // The state is still loaded here, and the page stayed where it was.
  await expect(page.locator('#result .ids li')).toHaveText(['shared']);
  expect(new URL(page.url()).pathname).toBe('/');
});

test('a link from a version with no playground says the answers may differ', async ({ page }) => {
  await page.goto('/' + fragment(state('0.0.8')));
  const banner = page.locator('#version-banner');
  await expect(banner).toContainText(`This link was made with pemba 0.0.8; this page runs pemba ${VERSION}.`);
  await expect(banner).toContainText('the answers here may differ');
  await expect(banner.getByRole('link')).toHaveCount(0);
  await expect(page.locator('#result .ids li')).toHaveText(['shared']);
});

test('a link from a /next/ build is never linked to a release', async ({ page }) => {
  let probed = false;
  await page.route('**/v/**', (route) => {
    probed = true;
    return route.fulfill({ status: 200, body: '' });
  });
  await page.goto('/' + fragment(state(`${VERSION}+abc1234`)));
  await expect(page.locator('#version-banner')).toContainText('the answers here may differ');
  expect(probed).toBe(false);
});

test('each example loads and runs', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#result .verdict')).toHaveText('allowed');
  const expected = {
    google_docs: /checks, all passed/,
    github_like: /checks, all passed/,
    agent_authz: /checks, all passed/,
  };
  for (const [name, verdict] of Object.entries(expected)) {
    await page.locator('#example').selectOption(name);
    await expect(page.locator('#tool-status')).toHaveText(`Loaded the ${name} example.`);
    await expect(page.locator('#assertions-result .verdict')).toHaveText(verdict);
    await expect(page.locator('#result .verdict')).not.toHaveText(/^Error/);
  }
  // The last one, agent_authz, shows the agent's tools/list.
  await expect(page.locator('#result .ids li')).toHaveText(['read_document', 'search_documents']);
});

test('export then import gives back the same editors and the same run', async ({ page }) => {
  await page.goto('/');
  await page.locator('#example').selectOption('github_like');
  await expect(page.locator('#assertions-result .verdict')).toHaveText(/checks, all passed/);
  const verdict = await page.locator('#assertions-result .verdict').textContent();

  const [first] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: 'Export YAML' }).click()]);
  expect(first.suggestedFilename()).toBe('pemba-playground.yaml');
  const yaml = readFileSync((await first.path()) ?? '', 'utf8');
  expect(yaml).toMatch(/^schema: \|-\n/);

  // Clear the editors, then import the file.
  await page.reload();
  await expect(page.locator('#result .verdict')).toHaveText('allowed');
  await page.locator('#import-file').setInputFiles({ name: 'github_like.yaml', mimeType: 'application/yaml', buffer: Buffer.from(yaml) });
  await expect(page.locator('#tool-status')).toHaveText('Imported github_like.yaml.');
  await expect(page.locator('#assertions-result .verdict')).toHaveText(verdict ?? '');

  const [second] = await Promise.all([page.waitForEvent('download'), page.getByRole('button', { name: 'Export YAML' }).click()]);
  expect(readFileSync((await second.path()) ?? '', 'utf8')).toBe(yaml);
});

test('a file that is not a fixture is refused and changes nothing', async ({ page }) => {
  await page.goto('/');
  await expect(page.locator('#result .verdict')).toHaveText('allowed');
  await page.locator('#import-file').setInputFiles({ name: 'bad.yaml', mimeType: 'application/yaml', buffer: Buffer.from('relationships: |-\n  doc:a#viewer@user:b\n') });
  await expect(page.locator('#notice')).toContainText('bad.yaml could not be imported');
  await expect(page.locator('#notice')).toContainText('The editors were not changed');
  await expect(page.locator('#text')).toHaveValue('document:readme#view@user:anne');
  await expect(editorText(page, 'relationships')).toContainText('group:eng#member@user:anne');
});
