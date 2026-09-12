import { mount } from 'svelte';
import './app.css';
import App from './App.svelte';
import { initApiBase } from './api/config';

// API base 单一配置点（决策 153④）：Tauri 壳可注入覆盖，默认同源相对路径。
initApiBase();

const app = mount(App, { target: document.getElementById('app')! });

export default app;
