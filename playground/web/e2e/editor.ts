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
