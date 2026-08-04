import { describe, it, expect } from 'vitest';
import { validateInput } from '../src/project-name';

describe('{{PROJECT_NAME}}', () => {
  describe('validateInput', () => {
    it('should return true for non-empty string', () => {
      expect(validateInput('hello')).toBe(true);
    });

    it('should return false for empty string', () => {
      expect(validateInput('')).toBe(false);
    });

    it('should return false for non-string input', () => {
      expect(validateInput(123)).toBe(false);
      expect(validateInput(null)).toBe(false);
      expect(validateInput(undefined)).toBe(false);
    });
  });
});
