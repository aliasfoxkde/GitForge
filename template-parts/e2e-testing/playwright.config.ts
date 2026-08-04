import { defineConfig, devices } from '@playwright/test';
import { resolve } from 'path';

/**
 * Playwright E2E Configuration
 *
 * Full-featured config for dark-factory E2E testing with:
 * - Multi-browser support (Chromium, Firefox, WebKit)
 * - Coverage collection via @vitest/coverage-v8
 * - Rich reporting (list, HTML, JSON)
 * - Debugging aids (traces, screenshots, videos)
 */
export default defineConfig({
	// ── Test Directory ────────────────────────────────────────────────────────
	testDir: './tests/e2e',

	// ── Output ────────────────────────────────────────────────────────────────
	outputDir: process.env.E2E_REPORT_DIR || './test-reports',

	// ── Timeout & Retries ─────────────────────────────────────────────────────
	timeout: parseInt(process.env.E2E_TIMEOUT || '30000', 10),
	retries: process.env.CI ? 2 : 1,

	// ── Reporter ──────────────────────────────────────────────────────────────
	// Multiple reporters for different needs
	reporter: [
		['list'],                      // Console output
		['html', { outputFolder: 'test-reports/html', open: 'never' }],
		['json', { outputFile: 'test-reports/results.json' }],
	],

	// ── Global Setup ──────────────────────────────────────────────────────────
	globalSetup: resolve(__dirname, 'tests/e2e/global-setup.ts'),
	globalTeardown: resolve(__dirname, 'tests/e2e/global-teardown.ts'),

	// ── Use workers wisely ────────────────────────────────────────────────────
	// In CI, respect the workflow parallelism; locally default to 1
	workers: process.env.CI
		? parseInt(process.env.PARALLEL_WORKERS || '2', 10)
		: 1,

	// ── Browser Configuration ─────────────────────────────────────────────────
	projects: [
		// Chromium
		{
			name: 'chromium',
			use: {
				...devices['Desktop Chrome'],
				baseURL: process.env.E2E_BASE_URL || 'http://localhost:3000',
				screenshot: 'only-on-failure',
				video: 'retain-on-failure',
				pdf: false,
				launchOptions: {
					// Slow down to help with flakiness detection
					args: ['--disable-dev-shm-usage'],
				},
			},
			retries: 2,
		},

		// Firefox
		{
			name: 'firefox',
			use: {
				...devices['Desktop Firefox'],
				baseURL: process.env.E2E_BASE_URL || 'http://localhost:3000',
				screenshot: 'only-on-failure',
				video: 'retain-on-failure',
				launchOptions: {
					args: ['--disable-dev-shm-usage'],
				},
			},
			retries: 2,
		},

		// WebKit
		{
			name: 'webkit',
			use: {
				...devices['Desktop Safari'],
				baseURL: process.env.E2E_BASE_URL || 'http://localhost:3000',
				screenshot: 'only-on-failure',
				video: 'retain-on-failure',
			},
			retries: 2,
		},
	],

	// ── Trace & Debug ──────────────────────────────────────────────────────────
	// Collect traces on first retry of failed tests (helps debug flakes)
	use: {
		trace: 'on-first-retry',
		debugWebSocket: false,
		actionTimeout: 10000,
		navigationTimeout: 30000,
	},

	// ── Coverage ───────────────────────────────────────────────────────────────
	// Note: Requires @vitest/coverage-v8 and coverage setup in test files
	coverage: process.env.E2E_COVERAGE === 'true'
		? {
				provider: 'v8',
				reporter: ['text', 'json', 'html'],
				reportsDirectory: './test-reports/coverage',
				exclude: [
					'node_modules/**',
					'dist/**',
					'tests/**',
					'**/*.config.ts',
					'**/global-*.ts',
				],
		  }
		: undefined,

	// ── Web Server (optional) ─────────────────────────────────────────────────
	webServer: process.env.CI
		? undefined   // In CI, app is already running
		: {
				command: 'npm run dev',
				url: 'http://localhost:3000',
				timeout: 120 * 1000,
				reuseExistingServer: true,
		  },

	// ── Paths ──────────────────────────────────────────────────────────────────
	snapshotDir: './tests/e2e/snapshots',

	// ── TypeScript ─────────────────────────────────────────────────────────────
	typescriptDir: './tests/e2e/tsconfig',

	// ── FullyQualified Error Messages ────────────────────────────────────────
	reportSlowTests: {
		threshold: 60 * 1000, // 60s
		reporters: ['list'],
	},
});
