import { useCounterStore } from '../stores/counterStore';

export function useCounter() {
  const { count, increment, decrement, reset } = useCounterStore();

  return {
    count,
    increment,
    decrement,
    reset,
    isZero: count === 0,
    isPositive: count > 0,
    isNegative: count < 0,
  };
}
