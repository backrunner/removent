import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  fullyParallel: true,
  workers: 1,
  use: { baseURL: 'http://localhost:48733', trace: 'retain-on-failure' },
  webServer: {
    command: 'pnpm preview --host localhost --port 48733',
    url: 'http://localhost:48733',
    reuseExistingServer: false
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    { name: 'webkit', use: { ...devices['Desktop Safari'] } }
  ]
});
