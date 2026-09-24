import { test, expect, type Page } from '@playwright/test';

async function ready(page: Page, path: string) {
  await page.goto(path);
  await expect(page.locator('html')).toHaveAttribute('data-svedocs-route', path.split('#')[0]);
}

test('connection preview supports keyboard navigation and localized destinations', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await ready(page, '/');
  await expect(page.locator('main')).toHaveCount(1);
  await page.getByRole('tab', { name: /Removent/ }).focus();
  await page.keyboard.press('ArrowRight');
  await expect(page.getByRole('tab', { name: /VNC/ })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('tabpanel')).toContainText('office.local:5900');
  await page.keyboard.press('End');
  await expect(page.getByRole('tabpanel')).toContainText('RDP · Client only');
  await expect(page.getByRole('link', { name: 'Explore this connection' })).toHaveAttribute('href', '/docs/rdp');
  await page.locator('.sd-scope-trigger').click();
  await page.getByRole('menuitemradio', { name: '简体中文' }).click();
  await expect(page).toHaveURL('/zh');
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
  await page.getByRole('tab', { name: /RDP/ }).click();
  await expect(page.getByRole('link', { name: '了解这种连接方式' })).toHaveAttribute('href', '/docs/zh/rdp');
  expect(errors).toEqual([]);
});

test('local search returns same-language documentation and restores focus', async ({ page }) => {
  await ready(page, '/docs');
  await page.locator('.sd-search-trigger').click();
  await page.getByRole('combobox').fill('relay');
  await expect(page.getByRole('option').first()).toBeVisible();
  for (const href of await page.getByRole('option').evaluateAll(nodes => nodes.map(n => n.getAttribute('href')))) {
    expect(href).not.toContain('/zh');
  }
  await page.keyboard.press('Escape');
  await expect(page.locator('.sd-search-trigger')).toBeFocused();
  await ready(page, '/docs/zh');
  await page.locator('.sd-search-trigger').click();
  await page.getByRole('combobox').fill('中继');
  await expect(page.getByRole('option').first()).toBeVisible();
  for (const href of await page.getByRole('option').evaluateAll(nodes => nodes.map(n => n.getAttribute('href')))) {
    expect(href).toContain('/zh');
  }
  await page.getByRole('option').first().click();
  await expect(page).toHaveURL(/\/docs\/zh/);
  await expect(page.locator('main h1')).toBeVisible();
});

test('system theme, explicit theme preference, and real product images stay aligned', async ({ page }, testInfo) => {
  await page.emulateMedia({ colorScheme: 'light', reducedMotion: 'reduce' });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await ready(page, '/');
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light');
  await expect(page.locator('.rv-image-light')).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('home-light.png'), fullPage: true });
  await page.locator('.sd-theme-toggle').click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await expect(page.locator('.rv-image-dark')).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('home-dark.png'), fullPage: true });
  await page.reload();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  const duration = await page.locator('.rv-button').first().evaluate(el => getComputedStyle(el).transitionDuration);
  expect(duration).toBe('0s');
});

test('mobile navigation, reading, and localized pages fit narrow screens', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await ready(page, '/zh');
  await expect(page.locator('.rv-menu-toggle')).toBeVisible();
  await expect(page.locator('.sd-scope-label-short')).toBeVisible();
  await expect(page.locator('.sd-scope-label-full')).toBeHidden();
  await expect(page.locator('.rv-search-control > svg')).toBeVisible();
  await page.locator('.sd-search-trigger').click();
  await expect(page.getByRole('combobox')).toBeVisible();
  await page.keyboard.press('Escape');
  await page.locator('.rv-menu-toggle').click();
  await expect(page.locator('.rv-mobile-menu')).toBeVisible();
  await page.locator('.rv-mobile-menu').getByRole('link', { name: '文档', exact: true }).click();
  await expect(page).toHaveURL('/docs/zh');
  await expect(page.locator('.rv-mobile-menu')).toHaveCount(0);
  await page.locator('.rv-menu-toggle').click();
  await page.locator('.rv-mobile-docs').getByRole('link', { name: '私有中继', exact: true }).click();
  await expect(page).toHaveURL('/docs/zh/relay');
  await expect(page.locator('h1')).toHaveText('私有中继');
  await page.screenshot({ path: testInfo.outputPath('mobile-docs.png'), fullPage: true });
  for (const width of [320, 390, 768]) {
    await page.setViewportSize({ width, height: 844 });
    for (const route of ['/', '/zh', '/download', '/zh/download', '/docs/relay', '/docs/zh/relay']) {
      await ready(page, route);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - innerWidth);
      expect(overflow, `${route} at ${width}px`).toBeLessThanOrEqual(1);
    }
  }
  await page.setViewportSize({ width: 390, height: 844 });
  await ready(page, '/zh');
  await page.locator('.rv-screenshot').scrollIntoViewIfNeeded();
  await expect.poll(() => page.locator('.rv-image-light').evaluate((el: HTMLImageElement) => el.naturalWidth)).toBeGreaterThan(0);
  await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'instant' }));
  await page.screenshot({ path: testInfo.outputPath('mobile-home.png'), fullPage: true });
});

test('docs anchors remain visible and code copy uses the displayed command', async ({ page, context, browserName }, testInfo) => {
  if (browserName === 'chromium') await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await ready(page, '/docs/hosting');
  await page.locator('.rv-docs-toc .sd-toc').getByRole('link', { name: 'Manage from the CLI', exact: true }).click();
  await expect(page).toHaveURL(/#manage-from-the-cli$/);
  await expect.poll(async () => page.locator('#manage-from-the-cli').evaluate(el => Math.round(el.getBoundingClientRect().top))).toBeGreaterThanOrEqual(76);
  const code = page.locator('.sd-code').first();
  await code.hover();
  await code.locator('.sd-code-copy').click();
  if (browserName === 'chromium') {
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    expect(copied).toContain('daemon service-status');
    expect(copied).toContain('REMOVENT_CLI=');
  }
  await page.screenshot({ path: testInfo.outputPath('desktop-docs.png'), fullPage: true });
});

test('grouped documentation and mobile contents keep bilingual guides reachable', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 1440, height: 1000 });
  await ready(page, '/docs');
  await expect(page.locator('.rv-docs-sidebar .rv-nav-group')).toHaveCount(4);
  await expect(page.locator('.rv-doc-shortcuts a')).toHaveCount(3);
  await page.locator('.rv-doc-shortcuts a[href="/docs/relay"]').click();
  await expect(page.locator('.rv-docs-sidebar a[aria-current="page"]')).toHaveText('Private relay');
  await expect(page.locator('.rv-prose, .sd-prose').first()).toContainText('check_interval_secs = 86400');
  await expect(page.locator('.rv-sidebar-release')).toHaveAttribute('href', /v0\.1\.3-beta\.1$/);
  await page.screenshot({ path: testInfo.outputPath('relay-light.png'), fullPage: false });
  await page.locator('.sd-theme-toggle').click();
  await page.screenshot({ path: testInfo.outputPath('relay-dark.png'), fullPage: false });
  await page.setViewportSize({ width: 390, height: 844 });
  await ready(page, '/docs/zh/relay');
  await page.locator('.rv-mobile-toc summary').click();
  await page.locator('.rv-mobile-toc').getByRole('link', { name: '日志位置与保留', exact: true }).click();
  await expect.poll(async () => page.getByRole('heading', { name: '日志位置与保留', exact: false }).evaluate(el => Math.round(el.getBoundingClientRect().top))).toBeGreaterThanOrEqual(76);
  await expect(page.locator('.rv-mobile-toc')).not.toHaveAttribute('open');
  await ready(page, '/docs/zh');
  await expect(page.locator('.rv-doc-shortcuts a[href="/docs/zh/relay"]')).toBeVisible();
});

test('download stays usable when the release lookup fails', async ({ page }, testInfo) => {
  await page.route('https://api.github.com/repos/backrunner/removent/releases/latest', route => route.fulfill({ status: 503, body: 'Unavailable' }));
  await ready(page, '/download');
  await expect(page.locator('.rv-release-meta')).toContainText('Live version lookup is unavailable.');
  await expect(page.locator('.rv-release .rv-primary')).toHaveAttribute('href', 'https://github.com/backrunner/removent/releases');
  await expect(page.getByRole('link', { name: /v0.1.3-beta.1/ })).toHaveAttribute('href', /v0\.1\.3-beta\.1$/);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.screenshot({ path: testInfo.outputPath('download-mobile.png'), fullPage: true });
});

test('static metadata, markdown, sitemap, assets, and download destinations are usable', async ({ page, request }) => {
  await ready(page, '/docs/zh/relay');
  await expect(page.locator('link[rel="canonical"]')).toHaveAttribute('href', 'https://removent.pwp.sh/docs/zh/relay/');
  await expect(page.locator('link[hreflang="en"]')).toHaveAttribute('href', 'https://removent.pwp.sh/docs/relay/');
  const og = await page.locator('meta[property="og:image"]').getAttribute('content');
  expect((await request.get(new URL(og!).pathname)).ok()).toBeTruthy();
  for (const route of ['/docs/zh/relay/index.md', '/index.md', '/llms.txt', '/llms-full.txt', '/sitemap.xml', '/robots.txt']) {
    const response = await request.get(route);
    expect(response.status(), route).toBe(200);
    expect((await response.text()).length).toBeGreaterThan(30);
  }
  await ready(page, '/download');
  const release = page.locator('.rv-release');
  await expect(release).toBeVisible();
  // Before (or without) the live GitHub lookup the button still lands on Releases.
  await expect(release.locator('.rv-primary')).toHaveAttribute('href', /^https:\/\/github\.com\/backrunner\/removent\/releases/);
  await ready(page, '/zh/download');
  await expect(page.locator('.rv-release')).toBeVisible();
  expect(await page.locator('a[href*="support"]').count()).toBe(0);
});

test('unknown routes show the themed error and a working way home', async ({ page }) => {
  await page.goto('/this-page-does-not-exist');
  await expect(page.locator('main')).toContainText('404');
  await expect(page.locator('.rv-header')).toBeVisible();
  await page.locator('.rv-header .rv-brand').click();
  await expect(page).toHaveURL('/');
  await expect(page.locator('h1')).toContainText('Your other Mac.');
});
