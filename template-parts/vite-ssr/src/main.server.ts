import React from 'react';
import { renderToString } from 'react-dom/server';
import { createServerRouter } from '@tanstack/react-router';
import { rootRoute } from './root';

export async function render(url: string) {
  const router = createServerRouter({ routeTree: rootRoute, url });
  const html = renderToString(<RouterProvider router={router} />);
  return { html, router };
}
