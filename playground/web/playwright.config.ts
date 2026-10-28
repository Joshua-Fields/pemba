import { defineConfig, devices } from '@playwright/test';

// End-to-end tests against the production build (`npm run build` first).
export default defineConfig({
  testDir: 'e2e',
  timeout: 30_000,
  reporter: process.env['CI'] ? 'github' : 'list',
  use: { baseURL: 'http://localhost:4173' },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'npm run preview',
    url: 'http://localhost:4173',
    reuseExistingServer: !process.env['CI'],
  },
});
