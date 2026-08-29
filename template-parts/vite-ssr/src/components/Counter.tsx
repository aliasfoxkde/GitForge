import { useState } from 'react';

export function Counter() {
  const [count, setCount] = useState(0);

  return (
    <div className="flex flex-col items-center gap-4 p-8 bg-zinc-900 rounded-xl">
      <span className="text-6xl font-bold tabular-nums">{count}</span>
      <div className="flex gap-2">
        <button
          onClick={() => setCount((c) => c - 1)}
          type="button"
          aria-label="Decrease count"
          className="px-4 py-2 bg-zinc-800 hover:bg-zinc-700 rounded-lg"
        >
          -
        </button>
        <button
          onClick={() => setCount(0)}
          type="button"
          className="px-4 py-2 bg-zinc-800 hover:bg-zinc-700 rounded-lg"
        >
          Reset
        </button>
        <button
          onClick={() => setCount((c) => c + 1)}
          type="button"
          aria-label="Increase count"
          className="px-4 py-2 bg-zinc-800 hover:bg-zinc-700 rounded-lg"
        >
          +
        </button>
      </div>
    </div>
  );
}
