import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// 决策 16：Vite + Svelte 5（runes）+ TS。无 SvelteKit。
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
      '^/(tasks|projects|providers|metrics|health)': {
        target: 'http://127.0.0.1:8787',
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
