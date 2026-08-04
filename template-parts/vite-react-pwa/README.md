# Vite React PWA Template

A production-ready ViteJS + React PWA template optimized for CSR-first deployments on Cloudflare Pages and Vercel.

## Quick Start

```bash
# Install dependencies
pnpm install

# Start development server
pnpm dev

# Build for production
pnpm build
```

## Features

- **React 19** with TypeScript
- **Vite 6** for fast builds and HMR
- **PWA** with Workbox (offline support, auto-updates)
- **TailwindCSS v4** for styling
- **Zustand 5** for state management
- **React Router 7** for routing
- **TanStack Query 6** for data fetching
- **Vitest** for unit and integration testing
- **Playwright** for E2E testing
- **Cloudflare Pages** deployment ready
- **Vercel** deployment ready

## Project Structure

```
src/
├── api/              # API client and endpoints
├── components/       # Reusable UI components
├── features/         # Feature-based modules
│   ├── auth/         # Authentication feature
│   └── counter/      # Counter feature (example)
├── hooks/            # Custom React hooks
├── lib/              # Utility functions
└── styles/           # Global styles
```

## Environment Variables

Create a `.env` file in the root directory:

```env
VITE_API_URL=https://api.example.com
VITE_APP_NAME=My App
```

| Variable       | Description                    | Default      |
|---------------|--------------------------------|--------------|
| `VITE_API_URL` | Base URL for API requests     | `/api`       |
| `VITE_APP_NAME`| Application name              | `Vite React PWA` |

## Scripts

| Command                | Description                          |
|-----------------------|--------------------------------------|
| `pnpm dev`            | Start development server            |
| `pnpm build`          | Build for production                 |
| `pnpm preview`        | Preview production build            |
| `pnpm test`           | Run all tests                        |
| `pnpm test:unit`      | Run unit tests                       |
| `pnpm test:integration` | Run integration tests              |
| `pnpm test:e2e`       | Run E2E tests                        |
| `pnpm lint`           | Run ESLint                           |
| `pnpm format`         | Format code with Prettier            |
| `pnpm typecheck`      | Run TypeScript type checking         |
| `pnpm coverage`       | Generate test coverage report        |

## Deployment

### Cloudflare Pages

1. Connect your GitHub repository to Cloudflare Pages
2. Set the build command: `pnpm build`
3. Set the output directory: `dist`
4. Add environment variables if needed:
   - `VITE_API_URL`
   - `VITE_APP_NAME`
5. Deploy!

Or use Wrangler for CLI deployment:

```bash
pnpm run build
wrangler pages deploy dist/
```

### Vercel

1. Import your GitHub repository to Vercel
2. Set the build command: `pnpm build`
3. Set the output directory: `dist`
4. Add environment variables if needed
5. Deploy!

The `vercel.json` includes rewrite rules for client-side routing.

## PWA Capabilities

- **Offline Support**: Service worker caches all static assets
- **Auto-Update**: New versions download automatically
- **Installable**: Add to home screen on mobile devices
- **Fast Loading**: Workbox precaches assets for instant loads

### PWA Icons

Replace the default icons in `public/icons/`:
- `icon-192.svg` - 192x192 app icon
- `icon-512.svg` - 512x512 app icon
- `maskable.svg` - Maskable icon for Android

## State Management

The template uses **Zustand** with persist middleware for client-side state:

```typescript
import { create } from 'zustand';
import { persist } from 'zustand/middleware';

interface CounterState {
  count: number;
  increment: () => void;
}

export const useCounterStore = create<CounterState>()(
  persist(
    (set) => ({
      count: 0,
      increment: () => set((s) => ({ count: s.count + 1 })),
    }),
    { name: 'counter-storage' }
  )
);
```

## Data Fetching

The template uses **TanStack Query** for server state:

```typescript
import { useQuery, useMutation } from '@tanstack/react-query';

const { data, isLoading } = useQuery({
  queryKey: ['users'],
  queryFn: () => fetch('/api/users').then(res => res.json()),
});
```

## Testing

### Unit & Integration Tests

```bash
pnpm test:unit
pnpm test:integration
```

### E2E Tests

```bash
pnpm test:e2e
```

Tests require a running dev server. Start with `pnpm dev` before running E2E tests.

## TypeScript

The template uses composite TypeScript projects:
- `tsconfig.json` - Base configuration
- `tsconfig.app.json` - App source files
- `tsconfig.node.json` - Node files (Vite config, etc.)

## Linting & Formatting

```bash
pnpm lint      # ESLint
pnpm format    # Prettier
```

## License

MIT
