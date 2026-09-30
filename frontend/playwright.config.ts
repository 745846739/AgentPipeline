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
  // `fullyParallel: false`：一个测试文件整体占一个 worker，文件内仍串行。
  // worker 数（决策 346）：旧注释「串行避免端口/资源互扰」里的端口理由早已不成立——
  // 后端是 `serve --port 0` 随机端口（harness 回读就绪行）、home 各自临时目录、且不再起
  // Vite dev server。CI 态放开到 4（4 vCPU runner，33 条 spec 按文件分给 4 个 worker），
  // 把最长的一杆近似除以 worker 数；本地仍 1——本机是 2 物理核，多 worker 只会互相挤兑，
  // 且决策 331 的本地口径是「窄跑」，用不到全量并行。
  fullyParallel: false,
  workers: process.env.CI ? 4 : 1,
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
