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

const app = mount(App, { target: document.getElementById('app')! });

export default app;
