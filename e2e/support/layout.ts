import { expect, type Page } from '@playwright/test';

/** Fails when the page scrolls sideways at its current size (REQ-UI-14). */
export async function expectNoSidewaysScroll(page: Page, label: string): Promise<void> {
  const widths = await page.evaluate(() => ({
    scroll: document.documentElement.scrollWidth,
    client: document.documentElement.clientWidth,
  }));
  expect(widths.scroll, `${label}: sideways scroll`).toBeLessThanOrEqual(widths.client);
}

/**
 * Fails when a pointer target is smaller than 24 by 24 CSS px (REQ-A11Y-07). A checkbox or radio
 * counts by the label row around it, which toggles it too.
 */
export async function expectTargetsAtLeast24(page: Page, label: string): Promise<void> {
  const small = await page.evaluate(() => {
    const minimum = 24;
    const found: string[] = [];
    const targets = document.querySelectorAll<HTMLElement>(
      'button, a[href], input, select, textarea, summary',
    );
    for (const target of targets) {
      let measured: HTMLElement = target;
      if (target instanceof HTMLInputElement && ['checkbox', 'radio'].includes(target.type)) {
        measured = target.closest('label') ?? target;
      }
      const box = measured.getBoundingClientRect();
      // Not rendered at this moment, for example inside a closed fold.
      if (box.width === 0 && box.height === 0) {
        continue;
      }
      if (box.width < minimum || box.height < minimum) {
        const name = target.getAttribute('aria-label') ?? target.textContent.trim();
        found.push(
          `${target.tagName.toLowerCase()} "${name}" ${String(box.width)} x ${String(box.height)}`,
        );
      }
    }
    return found;
  });
  expect(small, `${label}: targets under 24 x 24`).toEqual([]);
}
