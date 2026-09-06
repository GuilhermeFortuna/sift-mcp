import type { ApiError, Freshness, RegistrationInput } from './api/types';

export const emptyRepositoryForm: RegistrationInput = { name: '', repo_path: '', store_path: '', model_path: '', daemon_path: '' };

export function firstInvalidField(values: RegistrationInput): keyof RegistrationInput | null {
  for (const field of ['name', 'repo_path', 'store_path', 'model_path', 'daemon_path'] as const) if (!values[field].trim()) return field;
  return null;
}

export function mapRepositoryError(error: ApiError, values: RegistrationInput) {
  const field = error.code === 'duplicate_store' ? 'store_path' : error.code === 'invalid_registration' ? firstInvalidField(values) : null;
  return { field, message: error.message, values };
}

export type FreshnessTone = 'success' | 'warning' | 'danger' | 'neutral';
export function freshnessLabel(value: Pick<Freshness, 'head' | 'indexed_commit' | 'dirty' | 'unavailable_reason'>): { label: string; tone: FreshnessTone } {
  if (value.unavailable_reason || value.dirty === null) return { label: 'Unknown', tone: 'neutral' };
  if (!value.head && value.dirty === false) return { label: 'Unborn repository', tone: 'warning' };
  if (value.dirty) return { label: 'Working tree changed', tone: 'warning' };
  if (value.head && value.indexed_commit && value.head !== value.indexed_commit) return { label: 'Different commit', tone: 'danger' };
  if (value.head && value.indexed_commit === value.head) return { label: 'Commit aligned', tone: 'success' };
  return { label: 'No indexed commit', tone: 'neutral' };
}

export function formatBytes(value: number | null | undefined) {
  if (value === null || value === undefined) return 'Unavailable';
  if (value < 1024) return `${value} B`;
  const units = ['KB', 'MB', 'GB']; let current = value / 1024; let unit = units[0];
  for (let i = 1; current >= 1024 && i < units.length; i += 1) { current /= 1024; unit = units[i]; }
  return `${current.toFixed(current >= 10 ? 0 : 1)} ${unit}`;
}

export function formatTime(value: number | undefined) { return value ? new Date(value).toLocaleString() : 'Unavailable'; }

export function operationElapsed(startedAt: number, now: number) { return Math.max(0, (now - startedAt) / 1000); }
export function operationThroughput(done: number, elapsedSeconds: number) { return done > 0 && elapsedSeconds > 0 ? done / elapsedSeconds : null; }
export function providerLabel(provider: 'cuda' | 'cpu' | null | undefined) { return provider === 'cuda' ? 'CUDA' : provider === 'cpu' ? 'CPU' : 'Unavailable'; }
