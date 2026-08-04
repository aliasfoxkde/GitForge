import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import ssr from '@vitejs/plugin-ssr';
import tailwindcss from '@tailwindcss/vite';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    ssr({
      input: fileURLToPath(new URL('./src/main.server.ts', import.meta.url)),
    }),
  ],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  build: {
    rollupOptions: {
      output: {
        entryFileNames: 'server/[name].js',
      },
    },
  },
});
