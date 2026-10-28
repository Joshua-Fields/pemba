import { expect, test, type Page } from '@playwright/test';

// Written by `playground/wasm/tests/schema_error_spans.rs` from the library's own spans.
import expectations from './schema-errors.json' with { type: 'json' };
import { settle, settled, setText } from './editor';

// T6.5: schema errors and warnings, and rejected relationship lines, are marked inline at
// the engine's span, with code, message and help in the tooltip.

interface Case {
  fixture: string;
  code: string;
  schema: string;
  line: number;
  before: string;
  text: string;
  message: string;
  help: string | null;
}

const cases: Case[] = expectations;

interface Mark {
  line: number;
  start: number;
  text: string;
}

/**
 * The marks of `severity` in an editor, as (1-based line, UTF-16 column, text). Adjacent
 * pieces are joined: syntax highlighting can split one mark into several elements.
 */
async function marksIn(page: Page, editor: string, severity: 'error' | 'warning'): Promise<Mark[]> {
  return page.locator(`#${editor} .cm-content`).evaluate((content, cls) => {
    const lines = [...content.querySelectorAll(':scope > .cm-line')];
    const out: { line: number; start: number; text: string }[] = [];
    for (const el of content.querySelectorAll(`.${cls}`)) {
      const line = el.closest('.cm-line');
      if (!line) continue;
      const range = document.createRange();
      range.setStart(line, 0);
      range.setEndBefore(el);
      const piece = { line: lines.indexOf(line) + 1, start: range.toString().length, text: el.textContent ?? '' };
      const last = out.at(-1);
      if (last && last.line === piece.line && last.start + last.text.length === piece.start) {
        last.text += piece.text;
      } else {
        out.push(piece);
      }
    }
    return out;
  }, `cm-lintRange-${severity}`);
}

for (const c of cases) {
  test(`schema_errors/${c.fixture} shows ${c.code} at its span`, async ({ page }) => {
    await page.goto('/');
    await settled(page);
    await expect(page.locator('#result .verdict')).toHaveText('allowed');
    await settle(page, () => setText(page, 'schema', c.schema));

    const want = { line: c.line, start: c.before.length, text: c.text };
    await expect.poll(() => marksIn(page, 'schema', 'error')).toContainEqual(want);
    // Fail closed: the result pane shows the error, not an answer.
    await expect(page.locator('#result .verdict')).toHaveText(/^Error: /);

    // The tooltip of the mark's first character names the code, the message and the help.
    const line = page.locator('#schema .cm-content > .cm-line').nth(c.line - 1);
    const box = await line.boundingBox();
    if (!box) throw new Error('line not rendered');
    const x = await line.evaluate((el, offset) => {
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      let left = offset;
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const len = n.textContent?.length ?? 0;
        if (left < len) {
          const r = document.createRange();
          r.setStart(n, left);
          r.setEnd(n, left + 1);
          const rect = r.getBoundingClientRect();
          return rect.left + rect.width / 2;
        }
        left -= len;
      }
      throw new Error('offset past the line');
    }, c.before.length);
    await page.mouse.move(x, box.y + box.height / 2);
    const tooltip = page.locator('.cm-tooltip-lint');
    await expect(tooltip.locator('.diag-code').filter({ hasText: c.code })).toHaveCount(1);
    await expect(tooltip).toContainText(c.message);
    if (c.help !== null) await expect(tooltip.locator('.diag-help')).toContainText(c.help);
  });
}

test('a schema warning is marked, and the run still answers', async ({ page }) => {
  await page.goto('/');
  await settled(page);
  await expect(page.locator('#result .verdict')).toHaveText('allowed');
  const schema = 'definition user {}\n\ndefinition document {\n    relation viewer: user\n    permission view = viewer\n    /** dangling */\n}';
  // The answer stays `allowed`, so only a settled run proves it is this one.
  await settle(page, async () => {
    await setText(page, 'schema', schema);
    await setText(page, 'relationships', 'document:readme#viewer@user:anne');
  });
  await expect.poll(() => marksIn(page, 'schema', 'warning')).toEqual([
    { line: 6, start: 4, text: '/** dangling */' },
  ]);
  await expect(page.locator('#result .verdict')).toHaveText('allowed');
});

test('a relationship line that does not parse, or does not fit the schema, is marked', async ({ page }) => {
  await page.goto('/');
  await settled(page);
  await expect(page.locator('#result .verdict')).toHaveText('allowed');

  await settle(page, () => setText(page, 'relationships', 'group:eng#member@user:anne\n  not a tuple\ndocument:readme#viewer@user:bob'));
  await expect.poll(() => marksIn(page, 'relationships', 'error')).toEqual([
    { line: 2, start: 2, text: 'not a tuple' },
  ]);
  await expect(page.locator('#result .verdict')).toHaveText('Error: PARSE_ERROR');

  await settle(page, () => setText(page, 'relationships', 'group:eng#member@user:anne\ndocument:readme#editor@user:bob'));
  await expect.poll(() => marksIn(page, 'relationships', 'error')).toEqual([
    { line: 2, start: 0, text: 'document:readme#editor@user:bob' },
  ]);
  await expect(page.locator('#result .verdict')).toHaveText(/^Error: E_[A-Z_]+$/);

  // Fixed text clears the mark.
  await settle(page, () => setText(page, 'relationships', 'group:eng#member@user:anne'));
  await expect.poll(() => marksIn(page, 'relationships', 'error')).toEqual([]);
  await expect(page.locator('#result .verdict')).toHaveText(/^(allowed|denied)$/);
});
