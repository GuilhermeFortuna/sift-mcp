import { expect, test } from 'vitest';
import type { RegistrationInput } from './types';

test('registration contract carries only configured locations', () => {
  const registration: RegistrationInput = { name: 'repo', repo_path: '/repo', store_path: '/store', model_path: '/model', daemon_path: '/daemon' };
  expect(Object.keys(registration).sort()).toEqual(['daemon_path', 'model_path', 'name', 'repo_path', 'store_path']);
});
