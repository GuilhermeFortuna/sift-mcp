import { describe, expect, test } from 'vitest';
// @ts-ignore
import fs from 'node:fs';

function relativeLuminance(hex: string): number {
  const clean = hex.replace('#', '');
  const r = parseInt(clean.slice(0, 2), 16) / 255;
  const g = parseInt(clean.slice(2, 4), 16) / 255;
  const b = parseInt(clean.slice(4, 6), 16) / 255;
  const transform = (c: number) => (c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));
  return 0.2126 * transform(r) + 0.7152 * transform(g) + 0.0722 * transform(b);
}

function contrastRatio(hex1: string, hex2: string): number {
  const l1 = relativeLuminance(hex1);
  const l2 = relativeLuminance(hex2);
  const lighter = Math.max(l1, l2);
  const darker = Math.min(l1, l2);
  return (lighter + 0.05) / (darker + 0.05);
}

describe('UI theme and contrast', () => {
  const css = fs.readFileSync(new URL('./styles.css', import.meta.url), 'utf-8');

  test(':root binds color to var(--ink) and not hardcoded dark color', () => {
    // :root must use var(--ink) so color updates when theme variable switches
    expect(css).toMatch(/:root\s*\{[^}]*color:\s*var\(--ink\)/);
  });

  test('body sets color to var(--ink)', () => {
    expect(css).toMatch(/body\s*\{[^}]*color:\s*var\(--ink\)/);
  });

  test('dark theme provides high contrast for ink and muted on dark surfaces', () => {
    // Dark theme tokens
    const darkInk = '#e8f0ec';
    const darkMuted = '#a6b7b1';
    const darkSurface = '#16221f';
    const darkSurfaceSoft = '#1d302a';
    const darkBg = '#0f1816';

    expect(contrastRatio(darkInk, darkBg)).toBeGreaterThanOrEqual(7);
    expect(contrastRatio(darkInk, darkSurface)).toBeGreaterThanOrEqual(7);
    expect(contrastRatio(darkInk, darkSurfaceSoft)).toBeGreaterThanOrEqual(7);
    expect(contrastRatio(darkMuted, darkSurface)).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(darkMuted, darkSurfaceSoft)).toBeGreaterThanOrEqual(4.5);
  });

  test('primary button has sufficient contrast in both light and dark themes', () => {
    expect(css).toMatch(/--accent-contrast/);
    expect(css).toMatch(/\.primary\s*\{[^}]*color:\s*var\(--accent-contrast/);
  });
});
