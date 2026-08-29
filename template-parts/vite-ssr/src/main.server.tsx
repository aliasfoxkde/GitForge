import { renderToString } from 'react-dom/server';
import { RouterProvider, createRouter } from '@tanstack/react-router';
import { rootRoute } from './root';

export async function render(url: string) {
  const router = createRouter({ routeTree: rootRoute });
  await router.navigate({ to: url });
  const html = renderToString(<RouterProvider router={router} />);
  return { html, router };
}
