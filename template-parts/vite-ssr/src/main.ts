import React from 'react';
import ReactDOM from 'react-dom/client';
import { RouterProvider, createClientRouter } from '@tanstack/react-router';
import { rootRoute } from './root';

const routeConfig = createClientRouter({ routeTree: rootRoute });

const rootElement = document.getElementById('root')!;

ReactDOM.createRoot(rootElement).render(
  <RouterProvider router={routeConfig} />
);
