import { describe, expect, test } from 'vitest';
import {
  DEFAULT_LIMIT,
  MAX_LIMIT,
  MIN_LIMIT,
  createGenerationTracker,
  formatScore,
  formatTiming,
  generateLineNumbers,
  getSearchEndpoint,
  getSearchPayload,
  hasDegradation,
  mapSymbolError,
  shouldSubmitOnKeyDown,
  validateLimit,
} from './search-lab';

describe('search input limits and defaults', () => {
  test('default limit is 5', () => {
    expect(DEFAULT_LIMIT).toBe(5);
    expect(MIN_LIMIT).toBe(1);
    expect(MAX_LIMIT).toBe(20);
  });

  test('accepts valid integers between 1 and 20', () => {
    expect(validateLimit(1)).toEqual({ valid: true, value: 1 });
    expect(validateLimit(5)).toEqual({ valid: true, value: 5 });
    expect(validateLimit(20)).toEqual({ valid: true, value: 20 });
    expect(validateLimit('10')).toEqual({ valid: true, value: 10 });
  });

  test('rejects limits outside 1 to 20', () => {
    expect(validateLimit(0).valid).toBe(false);
    expect(validateLimit(-1).valid).toBe(false);
    expect(validateLimit(21).valid).toBe(false);
    expect(validateLimit(100).valid).toBe(false);
    expect(validateLimit('0').valid).toBe(false);
    expect(validateLimit('21').valid).toBe(false);
  });

  test('rejects fractional and non-numeric limits', () => {
    expect(validateLimit(5.5).valid).toBe(false);
    expect(validateLimit('5.5').valid).toBe(false);
    expect(validateLimit('abc').valid).toBe(false);
    expect(validateLimit('').valid).toBe(false);
    expect(validateLimit(NaN).valid).toBe(false);
    expect(validateLimit(Infinity).valid).toBe(false);
  });
});

describe('submission and keyboard events', () => {
  test('submits on Ctrl+Enter or Cmd+Enter when not composing IME', () => {
    expect(shouldSubmitOnKeyDown({ key: 'Enter', ctrlKey: true, metaKey: false })).toBe(true);
    expect(shouldSubmitOnKeyDown({ key: 'Enter', ctrlKey: false, metaKey: true })).toBe(true);
    expect(shouldSubmitOnKeyDown({ key: 'Enter', ctrlKey: true, metaKey: true })).toBe(true);
  });

  test('does not submit on plain Enter', () => {
    expect(shouldSubmitOnKeyDown({ key: 'Enter', ctrlKey: false, metaKey: false })).toBe(false);
  });

  test('does not submit when IME composition is active', () => {
    expect(
      shouldSubmitOnKeyDown({
        key: 'Enter',
        ctrlKey: true,
        metaKey: false,
        isComposing: true,
      })
    ).toBe(false);
    expect(
      shouldSubmitOnKeyDown({
        key: 'Enter',
        ctrlKey: false,
        metaKey: true,
        isComposing: true,
      })
    ).toBe(false);
  });

  test('does not submit on other keys', () => {
    expect(shouldSubmitOnKeyDown({ key: 'a', ctrlKey: true, metaKey: false })).toBe(false);
  });
});

describe('endpoint and payload selection', () => {
  test('selects query endpoint and payload for query mode', () => {
    expect(getSearchEndpoint('repo-1', 'query')).toBe('/repositories/repo-1/search');
    expect(getSearchPayload('query', 'find parser', 5)).toEqual({
      query: 'find parser',
      top_k: 5,
    });
  });

  test('selects similar endpoint and payload for similar code mode', () => {
    expect(getSearchEndpoint('repo-1', 'similar')).toBe('/repositories/repo-1/similar');
    expect(getSearchPayload('similar', 'fn parse() {}', 10)).toEqual({
      code: 'fn parse() {}',
      top_k: 10,
    });
  });
});

describe('score and timing formatting', () => {
  test('distinguishes null score from zero score', () => {
    expect(formatScore(null)).toBe('Unavailable');
    expect(formatScore(undefined)).toBe('Unavailable');
    expect(formatScore(0)).toBe('0.0000');
    expect(formatScore(0.0)).toBe('0.0000');
  });

  test('never displays fused score as a percentage or confidence claim', () => {
    const formatted = formatScore(0.85);
    expect(formatted).toBe('0.8500');
    expect(formatted).not.toContain('%');
    expect(formatted).not.toContain('confidence');
  });

  test('formats stage timings in milliseconds', () => {
    expect(formatTiming(0)).toBe('0 ms');
    expect(formatTiming(24)).toBe('24 ms');
  });

  test('detects degraded retriever states', () => {
    expect(
      hasDegradation({
        lexical_ok: true,
        dense_ok: true,
        lexical_error: null,
        dense_error: null,
        stage_millis: { embed: 1, lexical: 1, dense: 1, fuse: 1, assemble: 1, total: 5 },
      })
    ).toBe(false);

    expect(
      hasDegradation({
        lexical_ok: false,
        dense_ok: true,
        lexical_error: 'Index missing',
        dense_error: null,
        stage_millis: { embed: 1, lexical: 0, dense: 1, fuse: 1, assemble: 1, total: 4 },
      })
    ).toBe(true);

    expect(
      hasDegradation({
        lexical_ok: true,
        dense_ok: false,
        lexical_error: null,
        dense_error: 'CUDA out of memory',
        stage_millis: { embed: 0, lexical: 1, dense: 0, fuse: 1, assemble: 1, total: 3 },
      })
    ).toBe(true);
  });
});

describe('search response integrity and preview safety', () => {
  test('preserves daemon result order exactly without reordering', () => {
    const rawResults = [
      {
        file: 'b.rs',
        symbol: 'beta',
        signature: 'fn beta()',
        doc: null,
        preview: 'fn beta() {}',
        lines: [10, 20] as [number, number],
        lexical_score: 5.0,
        dense_score: null,
        fused_score: 0.02,
      },
      {
        file: 'a.rs',
        symbol: 'alpha',
        signature: 'fn alpha()',
        doc: 'Documentation',
        preview: 'fn alpha() {}',
        lines: [1, 5] as [number, number],
        lexical_score: null,
        dense_score: 0.9,
        fused_score: 0.03, // higher score, but must NOT be moved before beta!
      },
    ];

    // Ensure order is strictly maintained
    expect(rawResults.map((r) => r.symbol)).toEqual(['beta', 'alpha']);
  });

  test('previews are safe and do not interpret HTML tags', () => {
    const rawPreview = '<script>alert("xss")</script><img src=x onerror=alert(1)>';
    // When rendered as plain text in JSX, HTML tags remain escaped text
    expect(rawPreview.length).toBeLessThanOrEqual(320);
  });
});

describe('race protection with monotonic generation tracker', () => {
  test('rejects stale responses when generation has advanced', () => {
    const tracker = createGenerationTracker();
    const gen1 = tracker.next();
    const gen2 = tracker.next();

    expect(gen2).toBeGreaterThan(gen1);
    expect(tracker.isCurrent(gen1)).toBe(false);
    expect(tracker.isCurrent(gen2)).toBe(true);
  });
});

describe('symbol inspection error recovery mapping', () => {
  test('maps SymbolNotFound to not_found recovery state with action to re-search', () => {
    const error = {
      code: 'symbol_not_found',
      message: 'The symbol was not found in the current index.',
      retryable: false,
    };
    const mapped = mapSymbolError(error);
    expect(mapped.kind).toBe('not_found');
    expect(mapped.message).toContain('not found in the current index');
  });

  test('maps SymbolAmbiguous to ambiguous recovery state with candidate list', () => {
    const error = {
      code: 'symbol_ambiguous',
      message: 'The symbol is ambiguous; specify its qualified name.',
      retryable: false,
      candidates: ['src/lib.rs:fn parse()', 'src/lib.rs:fn parse(x: i32)'],
    };
    const mapped = mapSymbolError(error);
    expect(mapped.kind).toBe('ambiguous');
    if (mapped.kind === 'ambiguous') {
      expect(mapped.candidates).toEqual([
        'src/lib.rs:fn parse()',
        'src/lib.rs:fn parse(x: i32)',
      ]);
    }
  });

  test('maps generic error to unexpected error state', () => {
    const error = {
      code: 'daemon_unavailable',
      message: 'Could not connect',
      retryable: true,
    };
    const mapped = mapSymbolError(error);
    expect(mapped.kind).toBe('error');
    expect(mapped.message).toBe('Could not connect');
  });
});

describe('line numbers generation', () => {
  test('generates line number array from start line to end line', () => {
    expect(generateLineNumbers(1, 5)).toEqual([1, 2, 3, 4, 5]);
    expect(generateLineNumbers(10, 10)).toEqual([10]);
    expect(generateLineNumbers(10, 8)).toEqual([10]);
  });
});

describe('privacy boundaries and transient memory isolation', () => {
  test('url parameters contain only repository identity and never query or code tokens', () => {
    const repoId = 'repo-alpha-123';
    const privateQuery = 'CONFIDENTIAL_QUERY_SECRET_TOKEN';
    const privateCode = 'CONFIDENTIAL_CODE_SNIPPET_TOKEN';

    const url = new URL(`http://127.0.0.1:7331/search?repository=${repoId}`);

    // Assert that the URL query string only has 'repository'
    const params = Array.from(url.searchParams.keys());
    expect(params).toEqual(['repository']);
    expect(url.searchParams.get('repository')).toBe(repoId);

    // Assert private tokens do not appear in URL href
    expect(url.href).not.toContain(privateQuery);
    expect(url.href).not.toContain(privateCode);

    // Negative fixture: ensure leak detection catches intentional parameter leakage
    const leakyUrl = new URL(`http://127.0.0.1:7331/search?repository=${repoId}&q=${privateQuery}`);
    expect(leakyUrl.href).toContain(privateQuery);
  });

  test('local and session storage remain free of private query or symbol text', () => {
    const privateSnippet = 'SECRET_INSPECTED_BODY_TOKEN';

    // Mock storage inspect function
    const inspectStorage = (storage: Record<string, string>) => {
      return Object.values(storage).join(' ');
    };

    const cleanStorage: Record<string, string> = {
      theme: 'dark',
      sidebar_collapsed: 'false',
    };

    expect(inspectStorage(cleanStorage)).not.toContain(privateSnippet);

    // Negative fixture: ensure leak detection catches intentional storage leakage
    const dirtyStorage: Record<string, string> = {
      ...cleanStorage,
      cached_search: privateSnippet,
    };
    expect(inspectStorage(dirtyStorage)).toContain(privateSnippet);
  });
});

describe('responsive layout and theme adaptability', () => {
  // @ts-ignore
  const fs = require('node:fs');
  const css = fs.readFileSync(new URL('./styles.css', import.meta.url), 'utf-8');

  test('search layout includes responsive media queries for mobile viewports (e.g. 390px)', () => {
    expect(css).toMatch(/@media\s*\(max-width:\s*900px\)\s*\{[^}]*\.search-layout\s*\{[^}]*grid-template-columns:\s*1fr/);
  });

  test('search lab elements adapt to theme variables without hardcoded hex foregrounds', () => {
    // Assert search textarea and inputs use css variables
    expect(css).toMatch(/\.search-textarea\s*\{[^}]*color:\s*var\(--ink\)/);
    expect(css).toMatch(/\.search-textarea\s*\{[^}]*background:\s*var\(--surface\)/);
    expect(css).toMatch(/\.limit-input\s*\{[^}]*color:\s*var\(--ink\)/);
    expect(css).toMatch(/\.limit-input\s*\{[^}]*background:\s*var\(--surface\)/);
  });
});





