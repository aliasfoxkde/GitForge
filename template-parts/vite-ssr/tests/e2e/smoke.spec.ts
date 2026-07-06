import { test, expect } from '@playwright/test';

test.describe('Smoke Tests', () => {
  test('homepage loads without errors', async ({ page }) => {
    await page.goto('/');
    await expect(page.locator('h1')).toContainText('Vite SSR');
  });

  test('counter increments and decrements', async ({ page }) => {
    await page.goto('/');
    const counter = page.locator('span.text-6xl');
    await expect(counter).toHaveText('0');

    await page.click('button:has-text("+")');
    await expect(counter).toHaveText('1');

    await page.click('button:has-text("-")');
    await expect(counter).toHaveText('0');
  });

  test('counter reset works', async ({ page }) => {
    await page.goto('/');
    const counter = page.locator('span.text-6xl');

    await page.click('button:has-text("+")');
    await page.click('button:has-text("+")');
    await page.click('button:has-text("+")');
    await expect(counter).toHaveText('3');

    await page.click('button:has-text("Reset")');
    await expect(counter).toHaveText('0');
  });
});
