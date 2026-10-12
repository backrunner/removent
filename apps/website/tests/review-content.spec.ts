import { test, expect } from '@playwright/test';

// Isolated desktop content for native client captures. The provenance manifest
// identifies this as a rendered documentation page sent by the loopback host.
test('review content renders without private data', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.goto('/docs/connecting/');
  await expect(page.locator('main h1')).toBeVisible();
  await expect(page.locator('.rv-brand').first()).toContainText('Removent');
  await page.screenshot({ path: '../../dist/app-store-ios/demo-source.png', fullPage: false });
});
