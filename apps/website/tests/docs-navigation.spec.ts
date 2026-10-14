import { expect, test, type Locator, type Page } from '@playwright/test';

async function ready(page: Page, route: string) {
  await page.goto(route);
  await expect(page.locator('html')).toHaveAttribute('data-svedocs-route', route.replace(/\/$/, ''));
}

async function withinViewport(link: Locator, viewport: Locator) {
  const box = await link.boundingBox();
  const bounds = await viewport.boundingBox();
  expect(box).not.toBeNull();
  expect(bounds).not.toBeNull();
  expect(box!.y).toBeGreaterThanOrEqual(bounds!.y);
  expect(box!.y + box!.height).toBeLessThanOrEqual(bounds!.y + bounds!.height + 1);
}

test('short desktop windows keep both ends of navigation reachable without scrolling the article', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1440, height: 500 });
  await ready(page, '/docs/');
  const sidebar = page.locator('.rv-docs-sidebar');
  const area = sidebar.locator('.rv-scroll-area');
  const viewport = sidebar.locator('.rv-scroll-viewport');
  await expect(area).toHaveAttribute('data-scroll-above', 'false');
  await expect(area).toHaveAttribute('data-scroll-below', 'true');
  await expect(area).toHaveCSS('position', 'relative');
  expect(await area.evaluate(el => getComputedStyle(el, '::after').opacity)).toBe('1');
  const first = sidebar.getByRole('link', { name: 'Get started', exact: true });
  const last = sidebar.getByRole('link', { name: 'Build from source', exact: true });
  await withinViewport(first, viewport);
  const articleTop = await page.locator('h1').evaluate(el => el.getBoundingClientRect().top);
  const bounds = (await viewport.boundingBox())!;
  await page.mouse.move(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
  await page.mouse.wheel(0, 1500);
  await expect(area).toHaveAttribute('data-scroll-below', 'false');
  await withinViewport(last, viewport);
  expect(await area.evaluate(el => getComputedStyle(el, '::after').opacity)).toBe('0');
  await expect(area).toHaveAttribute('data-scroll-above', 'true');
  await page.mouse.wheel(0, 300);
  expect(await page.locator('h1').evaluate(el => el.getBoundingClientRect().top)).toBe(articleTop);
  const release = (await sidebar.locator('.rv-sidebar-release').boundingBox())!;
  expect(release.y + release.height).toBeLessThanOrEqual(500);
  await page.screenshot({ path: testInfo.outputPath('short-window-light.png') });

  // Keyboard focus is revealed above the fade, and selection never shifts the row.
  await first.focus();
  await withinViewport(first, viewport);
  await expect(area).toHaveAttribute('data-scroll-above', 'false');
  const before = await first.boundingBox();
  const layout = await first.evaluate((el: HTMLElement) => [el.offsetTop, el.offsetLeft, el.offsetWidth, el.offsetHeight]);
  await page.mouse.move(before!.x + before!.width / 2, before!.y + before!.height / 2);
  expect(await first.evaluate((el: HTMLElement) => [el.offsetTop, el.offsetLeft, el.offsetWidth, el.offsetHeight])).toEqual(layout);
  await expect(first).toHaveCSS('box-shadow', 'none');
  expect(await first.evaluate(el => getComputedStyle(el, '::after').display)).toBe('none');
  await page.locator('.sd-theme-toggle').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page.screenshot({ path: testInfo.outputPath('short-window-dark.png') });
  await page.setViewportSize({ width: 1440, height: 1200 });
  await expect(area).toHaveAttribute('data-scroll-above', 'false');
  await expect(area).toHaveAttribute('data-scroll-below', 'false');
});

test('long page contents scroll independently and follow the current section', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 500 });
  await ready(page, '/docs/relay/');
  const toc = page.locator('.rv-docs-toc');
  const area = toc.locator('.rv-scroll-area');
  const viewport = toc.locator('.rv-scroll-viewport');
  await expect(area).toHaveAttribute('data-scroll-below', 'true');
  const last = toc.getByRole('link', { name: 'Configuration reference', exact: true });
  await last.focus();
  await withinViewport(last, viewport);
  await last.press('Enter');
  await expect(page).toHaveURL(/#configuration-reference$/);
  await expect.poll(() => page.locator('#configuration-reference').evaluate(el => el.getBoundingClientRect().top)).toBeLessThan(200);
  await expect(last).toHaveClass(/sd-active/);
  await withinViewport(last, viewport);
  await expect(area).toHaveAttribute('data-scroll-below', 'false');
  // Direct document scrolling also reveals the current section in the contents list.
  await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'instant' }));
  const first = toc.getByRole('link', { name: 'Install on a VPS', exact: true });
  await expect(first).toHaveClass(/sd-active/);
  await expect(area).toHaveAttribute('data-scroll-above', 'false');
  await withinViewport(first, viewport);
});

test('mobile menu reveals the current guide and closes without moving the article', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 620 });
  await ready(page, '/docs/zh/building/');
  const articleTop = await page.locator('h1').evaluate(el => el.getBoundingClientRect().top);
  await page.locator('.rv-menu-toggle').click();
  const menu = page.locator('.rv-mobile-menu');
  const active = menu.locator('a[aria-current="page"]');
  await expect(active).toBeVisible();
  await withinViewport(active, menu.locator('.rv-scroll-viewport'));
  expect(await page.locator('h1').evaluate(el => el.getBoundingClientRect().top)).toBe(articleTop);
  const box = (await menu.boundingBox())!;
  expect(box.y + box.height).toBeLessThanOrEqual(620);
  await page.screenshot({ path: testInfo.outputPath('mobile-menu.png') });
  await page.locator('.rv-menu-toggle').click();
  await expect(menu).toHaveCount(0);
  await page.locator('.rv-mobile-toc summary').click();
  const contents = page.locator('.rv-mobile-toc .rv-scroll-viewport');
  const last = contents.locator('a').last();
  await last.focus();
  await withinViewport(last, contents);
  await last.press('Enter');
  await expect(page.locator('.rv-mobile-toc')).not.toHaveAttribute('open');
  expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth)).toBeLessThanOrEqual(1);
});

test('navigation fits intermediate tablet widths and zoom-sized viewports in both languages', async ({ page }) => {
  for (const route of ['/docs/relay/', '/docs/zh/relay/']) {
    await ready(page, route);
    for (const width of [320, 720, 800, 801, 900, 980, 981, 1024, 1101]) {
      await page.setViewportSize({ width, height: 650 });
      expect(await page.evaluate(() => document.documentElement.scrollWidth - innerWidth), `${route} at ${width}`).toBeLessThanOrEqual(1);
      if (width <= 980) await expect(page.locator('.rv-menu-toggle')).toBeVisible();
      else await expect(page.locator('.rv-desktop-nav')).toBeVisible();
    }
  }
});
