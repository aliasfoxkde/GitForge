import React from 'react';
import ReactDOM from 'react-dom/client';
import { RouterProvider, createClientRouter } from '@tanstack/react-router';
import { rootRoute } from './root';

const routeConfig = createClientRouter({ routeTree: rootRoute });

const rootElement = document.getElementById('root');

if (!rootElement) {
  throw new Error('Root element not found');
}

ReactDOM.createRoot(rootElement).render(
  <React.StrictMode>
    <RouterProvider router={routeConfig} />
  </React.StrictMode>
);
