import { describe, expect, test } from 'vitest';
import {
  emptyRepositoryForm,
  firstInvalidField,
  mapRepositoryError,
  freshnessLabel,
} from './repository-operations';

describe('repository form contracts', () => {
  test('finds the first missing required field in display order', () => {
    expect(firstInvalidField({ ...emptyRepositoryForm, name: 'Repo' })).toBe('repo_path');
  });

  test('maps duplicate stores to the store field without discarding input', () => {
    const input = { ...emptyRepositoryForm, name: 'Repo', store_path: '/stores/repo' };
    expect(mapRepositoryError({ code: 'duplicate_store', message: 'Already registered', retryable: false }, input)).toEqual({
      field: 'store_path',
      message: 'Already registered',
      values: input,
    });
  });
});

describe('freshness labels', () => {
  test('does not call matching commits fully fresh when the tree is dirty', () => {
    expect(freshnessLabel({ head: 'abc', indexed_commit: 'abc', dirty: true, unavailable_reason: null })).toEqual({
      label: 'Working tree changed',
      tone: 'warning',
    });
  });

  test('keeps an unborn repository distinct from an unavailable inspection', () => {
    expect(freshnessLabel({ head: null, indexed_commit: null, dirty: false, unavailable_reason: null }).label).toBe('Unborn repository');
    expect(freshnessLabel({ head: null, indexed_commit: null, dirty: null, unavailable_reason: 'inspection_unavailable' }).label).toBe('Unknown');
  });
});
