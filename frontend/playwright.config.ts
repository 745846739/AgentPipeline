/**
 * Playwright 配置（票 18，决策 144 / 150 / 151）：**只跑 Chromium**。
 *
 * 后端与前端 dev server 由 `e2e/harness.ts` 在每个用例的 `beforeAll` 里按需 spawn
 * （临时 home + mock LLM + `serve --port 0` 回读端口 + Vite 代理），因此不用
 * Playwright 的 `webServer`——那无法为每条用例隔离出独立的 `AGENTPIPELINE_HOME`。
 */

import { defineConfig, devices } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  testMatch: /.*\.spec\.ts$/,
  // 两条用例各自 spawn 后代进程与临时目录：串行避免端口/资源互相干扰。
  fullyParallel: false,
  workers: 1,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: [['list']],
  timeout: 180_000,
  expect: { timeout: 30_000 },
  use: {
    ...devices['Desktop Chrome'],
    baseURL: 'http://127.0.0.1',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
});
