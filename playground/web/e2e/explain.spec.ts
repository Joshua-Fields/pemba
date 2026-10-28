import { expect, test } from '@playwright/test';

// T6.7: a check shows its explain tree, and "copy as text" gives `CheckTrace::render`.
test.use({ permissions: ['clipboard-read', 'clipboard-write'] });

test('a check shows its explain tree, and copies it as text', async ({ page }) => {
  await page.goto('/');
  const result = page.locator('#result');
  await expect(result.locator('.verdict')).toHaveText('allowed');

  const rows = result.locator('.trace-tree .trace-row');
  await expect(rows).toHaveCount(4);
  // Durations are never shown.
  await expect(result.locator('.trace')).not.toContainText(/\d\s*(ns|µs|ms)\b/);

  await result.getByRole('button', { name: 'Copy as text' }).click();
  await expect(result.locator('.trace-copied')).toHaveText('copied');
  // A Windows clipboard reads back with CRLF line ends.
  const copied = (await page.evaluate(() => navigator.clipboard.readText())).replaceAll('\r\n', '\n');
  // The engine's `--explain` text, and the tree says the same, node for node.
  const lines = copied.split('\n');
  expect(lines[0]).toBe('allowed: document:readme#view@user:anne (revision 2)');
  const nodes = lines
    .slice(2)
    .filter((l) => l !== '')
    .map((l) => l.replace(/^(?:│ {3}| {4})*(?:├── |└── )?/u, ''));
  expect(nodes[2]).toBe('✓ group:eng#member  via document:readme#viewer@group:eng#member  ← group:eng#member@user:anne');
  await expect(rows).toHaveText(nodes);

  // A denied check has a tree too, never an error in its place; a lookup has none.
  await page.locator('#text').fill('document:readme#edit@user:anne');
  await page.getByRole('button', { name: 'Run' }).click();
  await expect(result.locator('.verdict')).toHaveText('denied');
  await expect(rows.first()).toHaveText('✗ document:readme#edit = owner');
  await page.locator('#kind').selectOption('lookup_resources');
  await expect(result.locator('.ids li')).toHaveText(['readme']);
  await expect(result.locator('.trace')).toHaveCount(0);
});
