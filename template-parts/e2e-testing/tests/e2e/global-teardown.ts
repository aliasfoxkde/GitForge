/**
 * Global Teardown — runs once after all E2E tests complete.
 * Use for cleanup, artifact collection, or session finalization.
 */

import { FullConfig } from '@playwright/test';

async function globalTeardown(config: FullConfig) {
	console.log('[E2E Global Teardown] Cleaning up E2E test session');

	// Print summary if in CI
	if (process.env.CI) {
		const reportDir = process.env.E2E_REPORT_DIR || './test-reports';
		console.log(`[E2E Global Teardown] Reports available at: ${reportDir}`);
	}

	console.log('[E2E Global Teardown] Teardown complete');
}

export default globalTeardown;
