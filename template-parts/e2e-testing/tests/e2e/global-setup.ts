/**
 * Global Setup — runs once before all E2E tests.
 * Use for authentication, test data seeding, or service setup.
 */

import { FullConfig } from '@playwright/test';

async function globalSetup(config: FullConfig) {
	console.log('[E2E Global Setup] Starting E2E test session');

	// Ensure report directories exist
	const fs = await import('fs');
	const reportDir = process.env.E2E_REPORT_DIR || './test-reports';
	const screenshotsDir = `${reportDir}/screenshots`;

	for (const dir of [reportDir, screenshotsDir]) {
		if (!fs.existsSync(dir)) {
			fs.mkdirSync(dir, { recursive: true });
		}
	}

	// Check if base URL is reachable (optional health check)
	const baseUrl = process.env.E2E_BASE_URL || 'http://localhost:3000';
	try {
		const response = await fetch(baseUrl, { method: 'HEAD' });
		if (!response.ok) {
			console.warn(`[E2E Global Setup] Warning: Base URL ${baseUrl} returned ${response.status}`);
		}
	} catch (err) {
		console.warn(`[E2E Global Setup] Warning: Could not reach ${baseUrl}: ${err}`);
	}

	console.log('[E2E Global Setup] Setup complete');
}

export default globalSetup;
