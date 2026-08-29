import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';

import { Counter } from '@/features/counter/components/Counter';
import { useCounterStore } from '@/features/counter/stores/counterStore';

describe('Counter', () => {
  beforeEach(() => {
    useCounterStore.getState().reset();
  });

  it('renders initial count of 0', () => {
    render(<Counter />);
    expect(screen.getByText('0')).toBeInTheDocument();
  });

  it('increments count', async () => {
    const user = userEvent.setup();
    render(<Counter />);
    await user.click(screen.getByRole('button', { name: '+' }));
    expect(screen.getByText('1')).toBeInTheDocument();
  });

  it('decrements count', async () => {
    const user = userEvent.setup();
    render(<Counter />);
    await user.click(screen.getByRole('button', { name: '-' }));
    expect(screen.getByText('-1')).toBeInTheDocument();
  });

  it('resets count', async () => {
    const user = userEvent.setup();
    render(<Counter />);
    await user.click(screen.getByRole('button', { name: '+' }));
    await user.click(screen.getByRole('button', { name: '+' }));
    await user.click(screen.getByRole('button', { name: '+' }));
    await user.click(screen.getByRole('button', { name: 'Reset' }));
    expect(screen.getByText('0')).toBeInTheDocument();
  });
});
