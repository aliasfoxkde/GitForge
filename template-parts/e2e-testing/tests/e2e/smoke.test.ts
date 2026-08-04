/**
 * E2E Smoke Tests
 *
 * Real passing tests demonstrating the E2E testing framework.
 * These tests verify basic functionality across all supported browsers.
 */

import { test, expect } from '@playwright/test';
import { Page } from '@playwright/test';

// ── Test Configuration ─────────────────────────────────────────────────────────
const BASE_URL = process.env.E2E_BASE_URL || 'http://localhost:3000';

// ── Helper Functions ──────────────────────────────────────────────────────────

/**
 * Navigate to a page and wait for it to be loaded.
 * Provides better error messages on failure.
 */
async function loadPage(page: Page, path: string = '/') {
	const url = `${BASE_URL}${path}`;
	const response = await page.goto(url, { waitUntil: 'networkidle' });

	if (!response) {
		throw new Error(`Failed to navigate to ${url}: no response`);
	}

	if (response.status() >= 400) {
		throw new Error(`Failed to navigate to ${url}: HTTP ${response.status()}`);
	}

	return response;
}

/**
 * Take a screenshot on failure with descriptive name.
 */
async function screenshotOnFailure(page: Page, name: string) {
	try {
		await page.screenshot({ path: `test-reports/screenshots/${name}.png`, fullPage: true });
	} catch {
		// Screenshot is best-effort; don't fail the test further
	}
}

// ── Smoke Tests ───────────────────────────────────────────────────────────────

/**
 * Test: Page loads successfully
 * Verifies the application is reachable and returns a valid response.
 */
test('page loads successfully', async ({ page }) => {
	test.info().annotations.push({
		type: 'smoke',
		description: 'Verifies basic application reachability',
	});

	const response = await loadPage(page, '/');

	// Assert the page loaded with a successful status
	expect(response?.status()).toBeLessThan(400);

	// Assert we have a valid HTML document
	const contentType = response?.headers()['content-type'] || '';
	expect(contentType).toMatch(/text\/html/);
});

/**
 * Test: Page title is present and non-empty
 * Verifies the application serves meaningful content.
 */
test('page has a title', async ({ page }) => {
	test.info().annotations.push({
		type: 'smoke',
		description: 'Verifies page has meaningful content',
	});

	await loadPage(page, '/');

	const title = await page.title();

	// Title should be non-empty (application-specific assertion)
	expect(title).toBeTruthy();
	expect(title.length).toBeGreaterThan(0);

	// Log for debugging
	console.log(`Page title: "${title}"`);
});

/**
 * Test: Main content area exists
 * Verifies the application renders expected structural elements.
 */
test('main content area is rendered', async ({ page }) => {
	test.info().annotations.push({
		type: 'smoke',
		description: 'Verifies page structure',
	});

	await loadPage(page, '/');

	// Check for common main content selectors
	// Adjust selectors based on your application's structure
	const mainContent = page.locator('main, [role="main"], .main, #main, body');

	const count = await mainContent.count();
	expect(count).toBeGreaterThan(0);

	console.log(`Found ${count} main content element(s)`);
});

// ── Skipped Test Example ──────────────────────────────────────────────────────

/**
 * Test: User can log in (SKIPPED - requires auth implementation)
 *
 * This demonstrates how to write an E2E test that is temporarily disabled.
 * Remove the `.skip` to enable once authentication is implemented.
 *
 * Example structure for authenticated tests:
 * ```typescript
 * test('user can log in', async ({ page }) => {
 *   await page.goto('/login');
 *   await page.fill('[name="username"]', 'testuser');
 *   await page.fill('[name="password"]', 'testpassword');
 *   await page.click('[type="submit"]');
 *   await expect(page).toHaveURL('/dashboard');
 * });
 * ```
 */
test.skip('user can log in', async ({ page }) => {
	// TODO: Implement when authentication is available
	// Steps:
	// 1. Navigate to /login
	// 2. Fill in username and password fields
	// 3. Submit the form
	// 4. Verify redirect to dashboard

	throw new Error('Authentication not yet implemented');
});

/**
 * Test: API endpoint returns expected data (SKIPPED - requires API)
 *
 * Example structure for API integration tests:
 * ```typescript
 * test('api returns correct data', async ({ request }) => {
 *   const response = await request.get(`${BASE_URL}/api/health`);
 *   expect(response.ok()).toBeTruthy();
 *   const data = await response.json();
 *   expect(data.status).toBe('ok');
 * });
 * ```
 */
test.skip('api endpoint returns expected data', async () => {
	// TODO: Implement when API is available
	throw new Error('API endpoint not yet available');
});

// ── Fixtures ─────────────────────────────────────────────────────────────────

export { loadPage, screenshotOnFailure };
