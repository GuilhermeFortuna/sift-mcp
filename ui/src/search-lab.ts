export const DEFAULT_LIMIT = 5;
export const MIN_LIMIT = 1;
export const MAX_LIMIT = 20;

export type SearchMode = 'query' | 'similar';

export interface LimitValidationResult {
  valid: boolean;
  value?: number;
  error?: string;
}

export function validateLimit(value: number | string): LimitValidationResult {
  if (typeof value === 'string') {
    const trimmed = value.trim();
    if (!trimmed || !/^-?\d+$/.test(trimmed)) {
      return { valid: false, error: 'Limit must be an integer between 1 and 20.' };
    }
    const parsed = Number.parseInt(trimmed, 10);
    return validateLimit(parsed);
  }

  if (typeof value !== 'number' || !Number.isInteger(value)) {
    return { valid: false, error: 'Limit must be an integer between 1 and 20.' };
  }

  if (value < MIN_LIMIT || value > MAX_LIMIT) {
    return { valid: false, error: `Limit must be between ${MIN_LIMIT} and ${MAX_LIMIT}.` };
  }

  return { valid: true, value };
}

export interface KeyDownEventLike {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  isComposing?: boolean;
}

export function shouldSubmitOnKeyDown(event: KeyDownEventLike): boolean {
  if (event.isComposing) {
    return false;
  }
  if (event.key !== 'Enter') {
    return false;
  }
  return Boolean(event.ctrlKey || event.metaKey);
}

export function getSearchEndpoint(repositoryId: string, mode: SearchMode): string {
  return `/repositories/${encodeURIComponent(repositoryId)}/${mode === 'query' ? 'search' : 'similar'}`;
}

export function getSearchPayload(
  mode: SearchMode,
  input: string,
  topK: number
): { query: string; top_k: number } | { code: string; top_k: number } {
  if (mode === 'query') {
    return { query: input, top_k: topK };
  }
  return { code: input, top_k: topK };
}

export function formatScore(score: number | null | undefined): string {
  if (score === null || score === undefined || Number.isNaN(score)) {
    return 'Unavailable';
  }
  return score.toFixed(4);
}

export function formatTiming(millis: number): string {
  return `${millis} ms`;
}

export interface DiagnosticsStatus {
  lexical_ok: boolean;
  dense_ok: boolean;
  lexical_error: string | null;
  dense_error: string | null;
  stage_millis?: unknown;
}

export function hasDegradation(diagnostics: DiagnosticsStatus): boolean {
  return (
    !diagnostics.lexical_ok ||
    !diagnostics.dense_ok ||
    Boolean(diagnostics.lexical_error) ||
    Boolean(diagnostics.dense_error)
  );
}

export interface GenerationTracker {
  next: () => number;
  current: () => number;
  isCurrent: (gen: number) => boolean;
}

export function createGenerationTracker(): GenerationTracker {
  let currentGen = 0;
  return {
    next: () => ++currentGen,
    current: () => currentGen,
    isCurrent: (gen: number) => gen === currentGen,
  };
}

export type MappedSymbolError =
  | { kind: 'not_found'; message: string }
  | { kind: 'ambiguous'; message: string; candidates: string[] }
  | { kind: 'error'; message: string };

export function mapSymbolError(error: { code: string; message: string; candidates?: string[] }): MappedSymbolError {
  if (error.code === 'symbol_not_found') {
    return {
      kind: 'not_found',
      message: error.message || 'The symbol was not found in the current index.',
    };
  }
  if (error.code === 'symbol_ambiguous') {
    return {
      kind: 'ambiguous',
      message: error.message || 'The symbol is ambiguous; select a qualified candidate.',
      candidates: error.candidates ?? [],
    };
  }
  return {
    kind: 'error',
    message: error.message || 'Failed to retrieve symbol body.',
  };
}

export function generateLineNumbers(start: number, end: number): number[] {
  if (end < start) {
    return [start];
  }
  const result: number[] = [];
  for (let i = start; i <= end; i++) {
    result.push(i);
  }
  return result;
}


