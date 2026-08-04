import { describe, it, expect, beforeEach } from 'vitest';
import { useCounterStore } from '@/features/counter/stores/counterStore';

describe('counterStore', () => {
  beforeEach(() => {
    useCounterStore.setState({ count: 0 });
  });

  it('has initial count of 0', () => {
    expect(useCounterStore.getState().count).toBe(0);
  });

  it('increments count', () => {
    const { increment } = useCounterStore.getState();
    increment();
    expect(useCounterStore.getState().count).toBe(1);
  });

  it('decrements count', () => {
    const { decrement } = useCounterStore.getState();
    decrement();
    expect(useCounterStore.getState().count).toBe(-1);
  });

  it('resets count to 0', () => {
    const { increment, reset } = useCounterStore.getState();
    increment();
    increment();
    increment();
    reset();
    expect(useCounterStore.getState().count).toBe(0);
  });

  it('handles multiple increments', () => {
    const { increment } = useCounterStore.getState();
    increment();
    increment();
    increment();
    increment();
    increment();
    expect(useCounterStore.getState().count).toBe(5);
  });
});
