import ReactDOM from 'react-dom/client';
import { RouterProvider, createRouter } from '@tanstack/react-router';
import { rootRoute } from './root';

const routeConfig = createRouter({ routeTree: rootRoute });

const rootElement = document.getElementById('root')!;

ReactDOM.createRoot(rootElement).render(
  <RouterProvider router={routeConfig} />
);
