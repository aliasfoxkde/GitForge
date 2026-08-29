import { test, expect } from '@playwright/test';

test.describe('Smoke Tests', () => {
  test('homepage loads successfully', async ({ page }) => {
    await page.goto('/');
    await expect(page).toHaveTitle(/Vite React PWA/);
  });

  test('counter increments on click', async ({ page }) => {
    await page.goto('/');
    const counter = page.locator('text=0').first();
    const incrementBtn = page.locator('button', { hasText: '+' });

    await expect(counter).toBeVisible();
    await incrementBtn.click();
    await expect(page.locator('text=1')).toBeVisible();
  });

  test('counter decrements on click', async ({ page }) => {
    await page.goto('/');
    const decrementBtn = page.locator('button', { hasText: '-' });

    await decrementBtn.click();
    await expect(page.locator('text=-1')).toBeVisible();
  });

  test('login form renders', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByPlaceholder('you@example.com')).toBeVisible();
    await expect(page.getByPlaceholder('Enter your password')).toBeVisible();
  });

  test('PWA manifest is accessible', async ({ page }) => {
    const response = await page.goto('/manifest.json');
    expect(response?.status()).toBe(200);
    const manifest = await response?.json();
    expect(manifest.name).toBe('Vite React PWA');
  });

  test('homepage exposes semantic landmarks and keyboard order', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByRole('main')).toBeVisible();
    await expect(page.getByRole('heading', { level: 1 })).toHaveText('Vite React PWA');
    await expect(page.getByRole('heading', { name: 'Login' })).toBeVisible();

    const email = page.getByLabel('Email');
    const password = page.getByLabel('Password');
    const submit = page.getByRole('button', { name: 'Sign In' });
    await email.focus();
    await page.keyboard.press('Tab');
    await expect(password).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(submit).toBeFocused();
    await submit.click();
    await expect(page.getByRole('alert')).toHaveText('Email is required');
  });
});
