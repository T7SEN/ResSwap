import { AxeBuilder } from '@axe-core/playwright';
import { expect, type Page } from '@playwright/test';

// WCAG 2.0, 2.1 and 2.2 at levels A and AA, as axe-core 4.14.0 tags its rules. No rule is turned
// off here; a proven false positive needs its own entry in docs/decisions.md first.
const WCAG_TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'wcag22aa'];

/** Runs the axe scan on the page as it is now and fails on any violation (REQ-A11Y-06). */
export async function expectNoAxeViolations(page: Page, label: string): Promise<void> {
  const results = await new AxeBuilder({ page }).withTags(WCAG_TAGS).analyze();
  const ids = results.violations.map((violation) => violation.id).join(', ');
  expect(results.violations, `${label}: ${ids}`).toEqual([]);
}
