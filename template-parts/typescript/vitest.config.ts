import { defineConfig } from 'vitest/config';
import { resolve } from 'path';

export default defineConfig({
	test: {
		globals: true,
		environment: 'node',
		include: ['tests/**/*.test.ts'],
		exclude: ['tests/e2e/**'],
		coverage: {
			provider: 'v8',
			reporter: ['text', 'json', 'html', 'lcov'],
			reportsDirectory: './coverage',
			exclude: [
				'node_modules/**',
				'dist/**',
				'tests/**',
				'**/*.config.ts',
				'**/.eslintrc.json',
			],
		},
		reporters: ['dot', 'spec'],
		typecheck: {
			enabled: true,
			tsconfig: './tsconfig.json',
		},
		testTimeout: 10000,
		hookTimeout: 10000,
	},
	resolve: {
		alias: {
			'@': resolve(__dirname, './src'),
		},
	},
});
