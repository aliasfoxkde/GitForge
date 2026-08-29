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

    const increaseButton = page.getByRole('button', { name: 'Increase count' });
    await increaseButton.focus();
    await increaseButton.press('Enter');
    await expect(counter).toHaveText('1');

    const decreaseButton = page.getByRole('button', { name: 'Decrease count' });
    await decreaseButton.focus();
    await decreaseButton.press('Enter');
    await expect(counter).toHaveText('0');
  });

  test('counter reset works', async ({ page }) => {
    await page.goto('/');
    const counter = page.locator('span.text-6xl');

    const increaseButton = page.getByRole('button', { name: 'Increase count' });
    await increaseButton.focus();
    await increaseButton.press('Enter');
    await increaseButton.press('Enter');
    await increaseButton.press('Enter');
    await expect(counter).toHaveText('3');

    const resetButton = page.getByRole('button', { name: 'Reset' });
    await resetButton.focus();
    await resetButton.press('Enter');
    await expect(counter).toHaveText('0');
  });
});
