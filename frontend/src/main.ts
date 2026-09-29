import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { capturePairingFromLocation, initApiBase } from './api/config';

// API base 单一配置点（决策 153④）：Tauri 壳可注入覆盖，默认同源相对路径。
initApiBase();

// 配对链接的消费（决策 182㉙，票 07；**由决策 191 修订**）：手机扫的那张二维码带 `?pair=…`，
// 在**任何请求发出之前**把它收进本地。放在 mount 之前是必需的——迟一步就会有请求裸奔一次，
// 而那次请求在非回环形态下会被 403 拒掉。
// **参数不再从地址栏抹掉**（191）：手机「添加到主屏幕」保存的就是这条 URL，而 iOS 的主屏
// web app 与 Safari 存储隔离——抹掉它等于让主屏图标每次都从零开始配对（详见 config.ts）。
capturePairingFromLocation();

// service worker 注册（pwa-webpush 02 票 03）：**唯一的注册点**，且只做注册——推送的
// 两件事（收报、点开）住在 `sw.ts` 里，这里不碰。
//
// 注册失败**不报错、不拦页面**：本应用不是「没有 SW 就不能用」，浏览器推送是纯增量的
// 第四通道（票面 30 号故事：不开就感知不到它的存在）。不支持 / 非安全上下文（比如
// 局域网的 http 地址）时这里静默跳过，设置页自会按 `pushFace` 说明原因。
if (typeof navigator !== 'undefined' && 'serviceWorker' in navigator && window.isSecureContext) {
  navigator.serviceWorker.register('/sw.js').catch(() => {
    // 注册失败（老浏览器 / 被策略拦）时只留一条足迹：页面照常。
  });
}

const app = mount(App, { target: document.getElementById('app')! });

export default app;
