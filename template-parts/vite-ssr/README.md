# Vite SSR Template

A server-side rendering (SSR) React application template using Vite, optimized for Vercel deployments.

## Quick Start

```bash
# Install dependencies
pnpm install

# Start development server
pnpm dev

# Run tests
pnpm test

# Type check
pnpm typecheck

# Lint
pnpm lint
```

## SSR vs CSR

This template uses **Server-Side Rendering (SSR)**, which means:

- **SSR (this template)**: The server renders the initial HTML on each request. Search engines see complete HTML content immediately. Better for SEO and initial page load performance.

- **CSR (Client-Side Rendering)**: The server sends a minimal HTML shell, then JavaScript renders the content in the browser. The initial HTML is nearly empty until JavaScript loads.

### When to Use SSR

| Use Case | Recommendation |
|----------|----------------|
| SEO-critical pages | SSR |
| Marketing sites | SSR |
| Dashboards (logged-in only) | CSR |
| SPAs with auth | Either (CSR often sufficient) |
| Performance on slow devices | SSR |

### Key Differences from CSR

- Server entry point: `src/main.server.ts`
- Client entry point: `src/main.ts` / `src/entry-client.tsx`
- Uses `@vitejs/plugin-ssr` for server builds
- Vercel config routes API requests to serverless functions

## Project Structure

```
├── src/
│   ├── main.ts           # Client entry point
│   ├── main.server.ts    # Server entry for SSR
│   ├── root.tsx          # Root component with routing
│   ├── routes.ts         # Route definitions
│   ├── api/
│   │   └── client.ts     # API client for server-side calls
│   ├── components/
│   │   └── Counter.tsx   # Example component
│   └── lib/
│       └── cn.ts         # Utility for className merging
├── api/
│   └── index.ts          # Vercel serverless API handler
├── public/
│   └── favicon.svg
├── tests/
│   ├── unit/             # Vitest unit tests
│   └── e2e/              # Playwright e2e tests
└── vercel.json           # Vercel deployment config
```

## Vercel Deployment

### Automatic Deployments

1. Push the project to GitHub
2. Import the project in Vercel
3. Vercel automatically detects the configuration

### Manual Deployments

```bash
# Install Vercel CLI
npm i -g vercel

# Deploy to preview
vercel

# Deploy to production
vercel --prod
```

### Environment Variables

Copy `.env.example` to `.env.local` for local development:

```bash
cp .env.example .env.local
```

Configure environment variables in Vercel dashboard under Settings > Environment Variables.

### Vercel Configuration

The `vercel.json` file configures:

- **API Routes**: `api/**/*.ts` files deploy as serverless functions
- **Static Build**: Client app builds to `dist/` directory
- **Routes**: `/api/*` routes to serverless functions, all other routes serve the SPA

## Scripts

| Command | Description |
|---------|-------------|
| `pnpm dev` | Start Vite dev server |
| `pnpm dev:server` | Start SSR dev server |
| `pnpm build` | Build client app |
| `pnpm build:server` | Build server bundle |
| `pnpm preview` | Preview production build |
| `pnpm test` | Run unit tests with Vitest |
| `pnpm test:e2e` | Run e2e tests with Playwright |
| `pnpm lint` | Run ESLint |
| `pnpm typecheck` | Run TypeScript type checking |

## Dependencies

### Production

- **React 19**: UI library
- **@tanstack/react-router**: File-based routing
- **zod**: Schema validation
- **clsx** & **tailwind-merge**: ClassName utilities

### Development

- **Vite 6**: Build tool
- **@vitejs/plugin-react**: React support
- **@vitejs/plugin-ssr**: SSR support
- **Tailwind CSS 4**: Styling
- **Vitest**: Unit testing
- **Playwright**: E2E testing
- **ESLint 9**: Linting
- **Prettier**: Code formatting

## Customization

### Adding Routes

Edit `src/routes.ts` to add new routes:

```tsx
export const newRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: '/new-page',
  component: function NewPage() {
    return <div>New Page</div>;
  },
});

const routeTree = rootRoute.addChildren([indexRoute, aboutRoute, newRoute]);
```

### Adding API Endpoints

Add files to `api/` directory:

```typescript
// api/data.ts
export default function handler(req: VercelRequest, res: VercelResponse) {
  res.json({ data: 'example' });
}
```

Access at `/api/data`.

### Styling

This template uses Tailwind CSS v4 with the Vite plugin. Edit `index.css` or use Tailwind classes directly in components.
