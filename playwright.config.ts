import { defineConfig } from '@playwright/test';
export default defineConfig({
  testDir: './tests/ui', timeout: 60_000, workers: 1,
  use: { headless: true, viewport: { width: 1440, height: 1000 }, trace: 'retain-on-failure' },
});
