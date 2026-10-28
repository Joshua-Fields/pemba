import { expect, test } from '@playwright/test';

// T6.4 smoke test: the built page loads the engine, runs a check, and shows the answer.
test('runs a check and shows the answer', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/');

  const result = page.locator('#result');
  // The page runs the default check on load.
  await expect(result.locator('.verdict')).toHaveText('allowed');
  await expect(result).toContainText('revision');

  await page.locator('#text').fill('document:readme#edit@user:anne');
  await page.getByRole('button', { name: 'Run' }).click();
  await expect(result.locator('.verdict')).toHaveText('denied');

  // An engine error is shown with its code, not as denied.
  await page.locator('#text').fill('document:readme#nope@user:anne');
  await page.getByRole('button', { name: 'Run' }).click();
  await expect(result.locator('.verdict')).toHaveText(/^Error: [A-Z_]+$/);
  await expect(result).not.toContainText('denied');

  await page.locator('#kind').selectOption('lookup_resources');
  await expect(result.locator('.ids li')).toHaveText(['readme']);

  expect(errors).toEqual([]);
});
