import { useCounterStore } from '../stores/counterStore';
import { Button } from '@/components/Button';
import { Card } from '@/components/Card';

export function Counter() {
  const { count, increment, decrement, reset } = useCounterStore();

  return (
    <Card className="max-w-sm mx-auto mt-8">
      <div className="flex flex-col items-center gap-4">
        <span className="text-6xl font-bold tabular-nums">{count}</span>
        <div className="flex gap-2">
          <Button onClick={decrement} variant="outline">-</Button>
          <Button onClick={reset} variant="ghost">Reset</Button>
          <Button onClick={increment} variant="outline">+</Button>
        </div>
      </div>
    </Card>
  );
}
