import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { capturePairingFromLocation, initApiBase } from './api/config';

// API base 单一配置点（决策 153④）：Tauri 壳可注入覆盖，默认同源相对路径。
initApiBase();

// 配对链接的消费（决策 182㉙，票 07）：手机扫的那张二维码带 `?pair=…`，在**任何请求
// 发出之前**把它收进本地、并从地址栏抹掉。放在 mount 之前是必需的——迟一步就会有请求
// 裸奔一次，而那次请求在非回环形态下会被 403 拒掉。
capturePairingFromLocation();

const app = mount(App, { target: document.getElementById('app')! });

export default app;
