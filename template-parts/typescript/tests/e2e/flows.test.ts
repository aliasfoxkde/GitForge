import { test, expect, describe } from '@playwright/test';

describe('E2E Resource Flows', () => {
	test.skip('full resource lifecycle from creation to deletion', async ({ page }) => {
		// Navigate to the application
		await page.goto('/');

		// Create a new resource
		await page.getByRole('button', { name: 'Create Resource' }).click();
		await page.getByLabel('Name').fill('E2E Test Resource');
		await page.getByLabel('Description').fill('Created by E2E test');
		await page.getByRole('button', { name: 'Submit' }).click();

		// Verify resource appears in list
		await expect(page.getByText('E2E Test Resource')).toBeVisible();

		// Navigate to resource detail
		await page.getByText('E2E Test Resource').click();

		// Verify resource details
		await expect(page.getByText('Created by E2E test')).toBeVisible();

		// Update the resource
		await page.getByRole('button', { name: 'Edit' }).click();
		await page.getByLabel('Description').fill('Updated description');
		await page.getByRole('button', { name: 'Save' }).click();

		// Verify update
		await expect(page.getByText('Updated description')).toBeVisible();

		// Delete the resource
		await page.getByRole('button', { name: 'Delete' }).click();
		await page.getByRole('button', { name: 'Confirm Delete' }).click();

		// Verify resource is gone
		await expect(page.getByText('E2E Test Resource')).not.toBeVisible();
	});

	test.skip('health check endpoint returns valid status', async ({ request }) => {
		const response = await request.get('/api/health');

		expect(response.ok()).toBeTruthy();

		const body = await response.json();
		expect(body.status).toBeTruthy();
		expect(body.timestamp).toBeTruthy();
		expect(body.version).toBeTruthy();
	});

	test.skip('handles concurrent resource creation', async ({ page }) => {
		await page.goto('/');

		// Create multiple resources rapidly
		for (let i = 0; i < 10; i++) {
			await page.getByRole('button', { name: 'Create Resource' }).click();
			await page.getByLabel('Name').fill(`Concurrent Resource ${i}`);
			await page.getByRole('button', { name: 'Submit' }).click();
		}

		// Verify all resources appear
		for (let i = 0; i < 10; i++) {
			await expect(page.getByText(`Concurrent Resource ${i}`)).toBeVisible();
		}
	});
});
