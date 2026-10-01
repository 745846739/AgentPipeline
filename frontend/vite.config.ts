import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// 决策 16：Vite + Svelte 5（runes）+ TS。无 SvelteKit。
// 代理目标默认本机 axum 8788；该端口被占用时用 `VITE_API_PROXY_TARGET` 覆盖
// （端口必须是后端 `serve --port` 实际绑定的那个）。
const apiTarget = process.env.VITE_API_PROXY_TARGET ?? 'http://127.0.0.1:8788';

export default defineConfig({
  plugins: [svelte()],
  build: {
    // 第二个入口：service worker（`src/sw.ts` → `dist/sw.js`，**不带 hash**）。
    // 理由：注册地址是约定（`/sw.js`），带 hash 就没人认得出它；而它的作用域由**地址**
    // 决定（根层 = 管全站），改地址等于换了作用域。内容变化由 `assets.rs` 的
    // `no-cache` 兜（决策 285：地址固定、内容会变的那一类必须每次复验）。
    rollupOptions: {
      input: {
        index: 'index.html',
        sw: 'src/sw.ts',
      },
      output: {
        // 只有 sw 这一个入口固定文件名，其余产物照旧交给 vite 的 hash 命名。
        entryFileNames: (chunk) => (chunk.name === 'sw' ? 'sw.js' : 'assets/[name]-[hash].js'),
      },
    },
  },
  // Svelte 5 组件测试：解析到浏览器构建（否则 mount 在 server build 上不可用）。
  resolve: {
    conditions: ['browser'],
  },
  server: {
    port: 5173,
    // 开发期把 API 透传到本机 axum（生产由 axum 直接托管 dist/，同源相对路径）。
    // 前端 api base 默认为同源相对路径（决策 153④），因此这里按路径前缀代理。
    proxy: {
      // skills / market 是票 09–16 新增的端点组：漏在这里的表现是「dev 下 404、打包后正常」
      // foreman（票 01 的对讲台三端点）同理；notify（离线通知一族——pwa-webpush 02 的
      // 订阅端点也走它）是同一件事，这一票才发现它一直漏着。
      '^/(tasks|projects|providers|stage-configs|skills|market|metrics|health|server-info|foreman|rtk|notify)':
        {
          target: apiTarget,
          changeOrigin: true,
        },
    },
  },
  test: {
    environment: 'jsdom',
    include: ['src/**/*.test.ts'],
    globals: true,
    // 纯函数 bench（决策 361，票 04）：与 `test.include` 分开点名——它**不进**
    // `npm test` 的断言集合（秒数类指标在 CI 上必然 flaky），只在 `npm run bench`
    // 里被显式收集。默认 include 会去捞 node_modules，故这里写死本仓的路径。
    benchmark: {
      include: ['src/**/*.bench.ts'],
    },
  },
});
