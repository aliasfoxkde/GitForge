import { createRootRoute, createRoute } from '@tanstack/react-router';
import { Outlet } from '@tanstack/react-router';
import { Counter } from './components/Counter';

export const rootRoute = createRootRoute({
  component: () => (
    <div className="min-h-screen bg-gray-950 text-gray-100 p-8">
      <Outlet />
    </div>
  ),
});

export const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/',
  component: function Index() {
    return (
      <div className="max-w-2xl mx-auto">
        <h1 className="text-4xl font-bold mb-8">Vite SSR</h1>
        <Counter />
      </div>
    );
  },
});

const routeTree = rootRoute.addChildren([indexRoute]);
export { routeTree };
