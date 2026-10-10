import { Routes, Route } from 'react-router-dom';
import { useAuth } from '@/features/auth';
import { Counter } from '@/features/counter';
import { LoginForm } from '@/features/auth/components/LoginForm';
import { Button } from '@/components/Button';
import { LoadingSpinner } from '@/components/LoadingSpinner';

export default function App() {
  const { user, isLoading, logout } = useAuth();

  if (isLoading) return <LoadingSpinner />;

  return (
    <div className="min-h-screen bg-background text-foreground">
      {/* WCAG 2.4.1: first focusable element bypasses repeated chrome. */}
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:bg-background focus:text-foreground focus:px-3 focus:py-2 focus:rounded-md focus:border focus:border-border"
      >
        Skip to content
      </a>
      <header className="flex items-center justify-between p-4 border-b border-border">
        <h1 className="text-xl font-bold">Vite React PWA</h1>
        {user ? (
          <div className="flex items-center gap-4">
            <span>{user.name}</span>
            <Button variant="outline" onClick={logout}>Logout</Button>
          </div>
        ) : (
          <LoginForm />
        )}
      </header>
      <main id="main-content" className="p-4">
        <Routes>
          <Route path="/" element={<Counter />} />
        </Routes>
      </main>
    </div>
  );
}
