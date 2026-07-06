import { describe, it, expect } from 'vitest';
import { cn } from '@/lib/cn';

describe('cn (classnames utility)', () => {
  it('merges class names', () => {
    const result = cn('foo', 'bar');
    expect(result).toBe('foo bar');
  });

  it('handles conditional classes', () => {
    const isActive = true;
    const result = cn('base', isActive && 'active');
    expect(result).toBe('base active');
  });

  it('filters falsy values', () => {
    const isEnabled = false;
    const result = cn('base', isEnabled && 'enabled');
    expect(result).toBe('base');
  });

  it('merges tailwind classes with conflict resolution', () => {
    const result = cn('px-2 px-4', 'py-2');
    expect(result).toBe('px-4 py-2');
  });

  it('handles array input', () => {
    const result = cn(['foo', 'bar']);
    expect(result).toBe('foo bar');
  });

  it('handles object input', () => {
    const result = cn({ foo: true, bar: false });
    expect(result).toBe('foo');
  });
});
