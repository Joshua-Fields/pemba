import { expect, test } from '@playwright/test';

import { settle, settled } from './editor';

// T6.4 smoke test: the built page loads the engine, runs a check, and shows the answer.
test('runs a check and shows the answer', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/');
  await settled(page);

  const result = page.locator('#result');
  // The page runs the default check on load.
  await expect(result.locator('.verdict')).toHaveText('allowed');
  await expect(result).toContainText('revision');

  await page.locator('#text').fill('document:readme#edit@user:anne');
  await settle(page, () => page.getByRole('button', { name: 'Run' }).click());
  await expect(result.locator('.verdict')).toHaveText('denied');

  // An engine error is shown with its code, not as denied.
  await page.locator('#text').fill('document:readme#nope@user:anne');
  await settle(page, () => page.getByRole('button', { name: 'Run' }).click());
  await expect(result.locator('.verdict')).toHaveText(/^Error: [A-Z_]+$/);
  await expect(result).not.toContainText('denied');

  await settle(page, () => page.locator('#kind').selectOption('lookup_resources'));
  await expect(result.locator('.ids li')).toHaveText(['readme']);

  expect(errors).toEqual([]);
});

// T6.9: the footer names the build (docs/11-playground.md §5).
test('the footer shows the version and commit', async ({ page }) => {
  await page.goto('/');
  const version = await page.locator('footer').getAttribute('data-version');
  expect(version).toMatch(/^\d+\.\d+\.\d+/);
  await expect(page.locator('#build-version')).toHaveText(version ?? '');
  await expect(page.locator('#build-commit')).toHaveText(/^[0-9a-f]{7,}$/);
});
