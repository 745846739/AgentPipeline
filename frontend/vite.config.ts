import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// 决策 16：Vite + Svelte 5（runes）+ TS。无 SvelteKit。
// 代理目标默认本机 axum 8787；该端口被占用时用 `VITE_API_PROXY_TARGET` 覆盖
// （端口必须是后端 `serve --port` 实际绑定的那个）。
const apiTarget = process.env.VITE_API_PROXY_TARGET ?? 'http://127.0.0.1:8787';

export default defineConfig({
  plugins: [svelte()],
  // Svelte 5 组件测试：解析到浏览器构建（否则 mount 在 server build 上不可用）。
  resolve: {
    conditions: ['browser'],
  },
  server: {
    port: 5173,
    // 开发期把 API 透传到本机 axum（生产由 axum 直接托管 dist/，同源相对路径）。
    // 前端 api base 默认为同源相对路径（决策 153④），因此这里按路径前缀代理。
    proxy: {
      '^/(tasks|projects|providers|stage-configs|metrics|health)': {
        target: apiTarget,
        changeOrigin: true,
      },
    },
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
    globals: true,
  },
});
