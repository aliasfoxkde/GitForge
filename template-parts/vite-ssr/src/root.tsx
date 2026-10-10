import { createRootRoute, createRoute } from '@tanstack/react-router';
import { Outlet } from '@tanstack/react-router';
import { Counter } from './components/Counter';

export const rootRoute = createRootRoute({
  component: () => (
    <div className="min-h-screen bg-gray-950 text-gray-100 p-8">
      {/* WCAG 2.4.1: first focusable element bypasses the page header. */}
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:bg-gray-950 focus:text-gray-100 focus:px-3 focus:py-2 focus:rounded-md focus:ring-2 focus:ring-gray-300"
      >
        Skip to content
      </a>
      {/* Landmarks over bare divs: generated projects extend the shell
          without retrofitting semantics (WCAG 1.3.1). */}
      <main id="main-content" className="max-w-2xl mx-auto">
        <Outlet />
      </main>
    </div>
  ),
});

export const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  component: function Index() {
    return (
      <>
        <h1 className="text-4xl font-bold mb-8">Vite SSR</h1>
        <Counter />
      </>
    );
  },
});

const routeTree = rootRoute.addChildren([indexRoute]);
export { routeTree };
