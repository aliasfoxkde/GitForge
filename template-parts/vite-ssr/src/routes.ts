import { createRootRoute, createRoute } from '@tanstack/react-router';
import { Counter } from '@/components/Counter';

export const rootRoute = createRootRoute({
  component: () => (
    <div className="min-h-screen bg-gray-950 text-gray-100 p-8">
      <a href="/" className="text-blue-400 hover:text-blue-300 mb-4 inline-block">
        Home
      </a>
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
        <p className="text-gray-400 mb-8">
          Server-side rendering with React and Vite on Vercel.
        </p>
        <Counter />
      </div>
    );
  },
});

export const aboutRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/about',
  component: function About() {
    return (
      <div className="max-w-2xl mx-auto">
        <h1 className="text-4xl font-bold mb-8">About</h1>
        <p className="text-gray-400">
          This is a server-side rendered React application powered by Vite.
        </p>
      </div>
    );
  },
});

const routeTree = rootRoute.addChildren([indexRoute, aboutRoute]);
export { routeTree };
