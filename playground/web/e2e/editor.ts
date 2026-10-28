import type { Page } from '@playwright/test';

/** Replaces an editor's text as a paste would, so no auto-indent or bracket closing runs. */
export async function setText(page: Page, editor: string, text: string): Promise<void> {
  const content = page.locator(`#${editor} .cm-content`);
  await content.click();
  await page.keyboard.press('ControlOrMeta+a');
  await content.evaluate((el, t) => {
    const data = new DataTransfer();
    data.setData('text/plain', t);
    el.dispatchEvent(new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }));
  }, text);
}

/** The id of the run the page painted last; 0 before the first. */
export async function runId(page: Page): Promise<number> {
  return page.evaluate(() => Number(document.body.dataset['runId'] ?? '0'));
}

/**
 * Waits until the page is idle with a run newer than `after` painted: the panes then show
 * the answer for the current inputs, not the one before them. `data-run` is set by
 * src/main.ts for this.
 */
export async function settled(page: Page, after = 0): Promise<void> {
  await page.waitForFunction(
    (n) => document.body.dataset['run'] === 'idle' && Number(document.body.dataset['runId'] ?? '0') > n,
    after,
  );
}

/** Does `action`, which changes the inputs, then waits for the run of the new inputs. */
export async function settle(page: Page, action: () => Promise<unknown>): Promise<void> {
  const before = await runId(page);
  await action();
  await settled(page, before);
}
