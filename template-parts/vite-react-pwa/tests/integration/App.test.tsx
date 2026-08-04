import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { BrowserRouter } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import App from '@/App';

describe('App', () => {
  const createWrapper = () => {
    const queryClient = new QueryClient({
      defaultOptions: {
        queries: { retry: false },
        mutations: { retry: false },
      },
    });

    return ({ children }: { children: React.ReactNode }) => (
      <QueryClientProvider client={queryClient}>
        <BrowserRouter>{children}</BrowserRouter>
      </QueryClientProvider>
    );
  };

  it('renders the app title', () => {
    const Wrapper = createWrapper();
    render(<App />, { wrapper: Wrapper });
    expect(screen.getByText('Vite React PWA')).toBeInTheDocument();
  });

  it('renders the counter on home page', () => {
    const Wrapper = createWrapper();
    render(<App />, { wrapper: Wrapper });
    expect(screen.getByText('0')).toBeInTheDocument();
  });

  it('renders login form when not authenticated', () => {
    const Wrapper = createWrapper();
    render(<App />, { wrapper: Wrapper });
    expect(screen.getByPlaceholderText('you@example.com')).toBeInTheDocument();
  });
});
