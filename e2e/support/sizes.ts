/** The main window's default and minimum sizes in logical pixels (REQ-UI-14). */
export const SIZES = [
  { width: 900, height: 640 },
  { width: 720, height: 560 },
] as const;

/** The keep-this-mode window's size in logical pixels; it does not resize (REQ-UI-06). */
export const KEEP_MODE_SIZE = { width: 440, height: 240 } as const;

/** Both Windows app modes, set through Playwright's colorScheme (REQ-UI-12). */
export const THEMES = ['light', 'dark'] as const;

export type Size = (typeof SIZES)[number];
export type Theme = (typeof THEMES)[number];

/** The size part of a screenshot name, for example 900x640. */
export function sizeLabel(size: { readonly width: number; readonly height: number }): string {
  return `${String(size.width)}x${String(size.height)}`;
}
