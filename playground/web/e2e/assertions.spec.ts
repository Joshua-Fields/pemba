import { expect, test, type Page } from '@playwright/test';

// Written by `playground/wasm/tests/assertion_corpus.rs`: every `testdata/fixtures` file,
// split into the three editors, and how many checks the native runner counts in it.
import corpus from './corpus.json' with { type: 'json' };
import { setText } from './editor';

// T6.6 (docs/11-playground.md §6 layer 3): the assertions editor runs the three editors as
// one fixture, `pemba test` semantics, and marks each line with its result.

// Tall enough that CodeMirror renders every line of the longest corpus assertions.
test.use({ viewport: { width: 1280, height: 4000 } });

async function load(page: Page, parts: { schema: string; relationships: string; assertions: string }) {
  await page.goto('/');
  await page.addStyleTag({ content: '#assertions .cm-editor { max-height: none; }' });
  await expect(page.locator('#assertions-result .verdict')).toHaveText(/passed|fail/);
  await setText(page, 'schema', parts.schema);
  await setText(page, 'relationships', parts.relationships);
  await setText(page, 'assertions', parts.assertions);
}

const marks = (page: Page, state: string) => page.locator(`#assertions .cm-assert-gutter .assert-${state}`);

for (const c of corpus) {
  test(`${c.fixture}: every assertion passes in the page`, async ({ page }) => {
    await load(page, c);
    const n = c.checked === 1 ? '1 check' : `${c.checked} checks`;
    await expect(page.locator('#assertions-result .verdict')).toHaveText(`${n}, all passed`);
    await expect(marks(page, 'pass')).toHaveCount(c.checked);
    await expect(marks(page, 'fail')).toHaveCount(0);
    await expect(marks(page, 'skipped')).toHaveCount(0);
  });
}

const SCHEMA = 'definition user {}\ndefinition doc {\n    relation viewer: user\n    permission view = viewer\n}';
const RELATIONSHIPS = 'doc:a#viewer@user:anne';

test('a failing assertion is marked on its line with the runner message', async ({ page }) => {
  await load(page, {
    schema: SCHEMA,
    relationships: RELATIONSHIPS,
    assertions: 'assertions:\n  assertTrue:\n    - doc:a#view@user:anne\n    - doc:a#view@user:bob\n',
  });
  await expect(page.locator('#assertions-result .verdict')).toHaveText('1 failure (2 checks)');
  await expect(page.locator('#assertions-result .failures')).toContainText('assertTrue doc:a#view@user:bob');
  await expect(page.locator('#assertions-result .failures')).toContainText('expected true, but engine says false');
  const gutter = page.locator('#assertions .cm-assert-gutter .cm-gutterElement');
  await expect(gutter.filter({ has: page.locator('.assert-pass') })).toHaveText(['✓']);
  await expect(marks(page, 'fail')).toHaveAttribute('title', /failed: expected true, but engine says false/);
  // The failing line is the fourth: its gutter mark and its inline mark are both there.
  const fourth = page.locator('#assertions .cm-content > .cm-line').nth(3);
  await expect(fourth.locator('.cm-lintRange-error')).toHaveText('doc:a#view@user:bob');
});

test('an unknown key is a failure, marked at its line', async ({ page }) => {
  await load(page, {
    schema: SCHEMA,
    relationships: RELATIONSHIPS,
    assertions: 'assertions:\n  assertTrue:\n    - doc:a#view@user:anne\n  assertTure:\n    - doc:a#view@user:bob\n',
  });
  await expect(page.locator('#assertions-result .verdict')).toHaveText(/^1 failure/);
  await expect(page.locator('#assertions-result .failures')).toContainText('unknown field `assertTure`');
  // Nothing ran, so nothing is marked as passing.
  await expect(marks(page, 'pass')).toHaveCount(0);
  await expect(marks(page, 'fail')).toHaveAttribute('title', /line 4 column 3: unknown field `assertTure`/);
  const fourth = page.locator('#assertions .cm-content > .cm-line').nth(3);
  await expect(fourth.locator('.cm-lintRange-error')).not.toHaveCount(0);
});

test('an engine error in the schema is shown with its code, not as a pass', async ({ page }) => {
  await load(page, {
    schema: 'definition doc { relation viewer: nope }',
    relationships: '',
    assertions: 'assertions:\n  assertTrue:\n    - doc:a#viewer@user:anne\n',
  });
  await expect(page.locator('#assertions-result .verdict')).toHaveText(/^1 failure/);
  await expect(page.locator('#assertions-result .failures')).toContainText('E_UNKNOWN_TYPE');
  await expect(marks(page, 'pass')).toHaveCount(0);
});
