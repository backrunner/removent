import { test, expect, type Page } from '@playwright/test';

async function verification(page: Page) {
  await page.addInitScript(() => {
    const host = window as unknown as { turnstile: unknown; expireSupport: () => void };
    host.turnstile = {
      render(_element: HTMLElement, options: { callback: (value: string) => void; 'expired-callback': () => void }) {
        host.expireSupport = options['expired-callback'];
        queueMicrotask(() => options.callback('mock-verified-token'));
        return 'mock-widget';
      }, reset() {}, remove() {}
    };
  });
  await page.route('https://challenges.cloudflare.com/**', route => route.fulfill({ contentType: 'application/javascript', body: '' }));
}

for (const locale of ['en', 'zh']) test(`support form submits ${locale} requests and shows the ticket reference`, async ({ page }) => {
  await verification(page);
  const requests: Record<string, string>[] = [];
  await page.route('**/api/support', async route => {
    requests.push(route.request().postDataJSON());
    await route.fulfill({ status: 201, json: { ticketId: 'reference-123' } });
  });
  await page.goto(locale === 'zh' ? '/zh/support/' : '/support/');
  await expect(page.locator('.support-panel')).toBeVisible();
  await page.locator('input[name="email"]').fill('reader@example.com');
  await page.locator('input[name="subject"]').fill('Records question');
  await page.locator('textarea[name="content"]').fill('How do I synchronize records?');
  await page.locator('input[value="question"]').check();
  await page.locator('.support-send').click();
  await expect(page.locator('.support-success')).toContainText('reference-123');
  await expect(page.locator('.support-success h2')).toBeFocused();
  expect(requests).toHaveLength(1);
  expect(requests[0]).toMatchObject({ locale, topic: 'question', email: 'reader@example.com', turnstileToken: 'mock-verified-token' });
  await expect(page.locator('input[name="email"]')).toHaveValue('');
});

test('support form preserves the message and prevents duplicate submissions after an uncertain result', async ({ page }) => {
  await verification(page);
  let requests = 0;
  await page.route('**/api/support', async route => { requests++; await route.fulfill({ status: 502, json: { error: 'uncertain' } }); });
  await page.goto('/zh/support/');
  await page.locator('input[name="email"]').fill('reader@example.com');
  await page.locator('input[name="subject"]').fill('测试问题');
  await page.locator('textarea[name="content"]').fill('完整的复现步骤。');
  await page.locator('.support-send').click();
  await expect(page.getByRole('alert')).toContainText('避免重复提交');
  await expect(page.locator('textarea')).toHaveValue('完整的复现步骤。');
  await expect(page.locator('.support-send')).toBeDisabled();
  expect(requests).toBe(1);
});

test('support form keeps verification recovery and help guides available on small screens', async ({ page }) => {
  await page.route('https://challenges.cloudflare.com/**', route => route.abort());
  for (const width of [320, 390, 768, 1440]) {
    await page.setViewportSize({ width, height: 900 });
    await page.goto('/zh/support/');
    await expect(page.getByRole('alert')).toContainText('安全验证未能加载');
    await expect(page.locator('.support-send')).toBeDisabled();
    await expect(page.getByRole('button', { name: '重新验证' })).toBeVisible();
    await expect(page.getByRole('heading', { name: '发送前可以先检查' })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  }
});

test('an expired verification token disables submission', async ({ page }) => {
  await verification(page);
  await page.goto('/support/');
  await expect(page.locator('.support-send')).toBeEnabled();
  await page.evaluate(() => (window as unknown as { expireSupport: () => void }).expireSupport());
  await expect(page.locator('.support-send')).toBeDisabled();
});
