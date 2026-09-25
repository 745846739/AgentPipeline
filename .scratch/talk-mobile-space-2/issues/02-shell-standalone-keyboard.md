# 02: 浏览器壳让位：主屏幕 standalone + 键盘收缩

**叠:** A（不动对讲台版面一行代码；动的是 index.html / 静态产物 / 指引文案）

**来源:** 票 01 落的决策；spec.md 第一节的账。上一轮（talk-mobile-space）收口后，版面内无大头可省——本票拿的是**浏览器壳**那 60~90px。

**What to build:** 手机把本应用「添加到主屏幕」后以**独立窗口（standalone）**打开：Safari 的地址栏与底栏整体让位，对话区约 +60~90px——且**看板、任务详情、设置全站受益**。安卓 Chrome/WebView 上聚焦输入框时键盘弹出**版面真收缩**（`interactive-widget=resizes-content`），钉底的东西不再被键盘压住、对话区照常可读可滚。四块工作：

1. **manifest**：`display: standalone`、名称 / 短名、主题色取像素主题既有 token；经桌面机生成的访问页与 127.0.0.1 两条路径都能取到（静态产物，无端点改动——若 LAN 令牌路径下取不到要如实记录并就地解决）。
2. **iOS meta + 图标**：`apple-mobile-web-app-capable` 一组 meta、`apple-touch-icon`；正式图标现仓里**没有**（只有 `.scratch/shots/icon-preview.png` 预览图），需从像素主题的既有标产出 ≥192/512 与 apple-touch 尺寸。底部安全区由既有 `--safeb` / `viewport-fit=cover` 基建承接，验证输入坞不与 Home 指示条重叠。
3. **键盘收缩**：viewport meta 追加 `interactive-widget=resizes-content`（iPhone 不认、由 WebKit 自理；安卓生效即达标）。
4. **指引对齐**：对讲台配对说明里「若已添加到主屏幕，换过令牌后要重新添加一次」那句按 01 定的口径补全（standalone 打开即无浏览器 chrome），与行为一致、不写空头指引。

**Blocked by:** 01（定性「壳层让位」与指引文案口径在那条决策里）

**Status:** done（manifest / 图标 / meta / 指引文案 / e2e 全落；三项真机目检待验——见实现记录）

- [ ] iPhone（Safari，经扫码 / LAN 地址）：添加到主屏幕后从图标打开为 standalone——无地址栏 / 底栏，底部安全区正确，输入坞钉底不与 Home 指示条重叠
- [ ] Android Chrome：聚焦输入框键盘弹出后，对话区仍可滚、输入坞与发送钮可见（`resizes-content` 生效）；iPhone 上无回归
- [ ] manifest 与图标（192 / 512 / apple-touch）进静态产物，两条访问路径可取到；图标不像素风破功
- [ ] 配对指引文案与 standalone 实际行为一致（含「换过令牌要重新添加」既有提醒不丢）
- [ ] e2e：viewport meta 内容含 `interactive-widget=resizes-content`；manifest 链接存在且资源可取
- [ ] 桌面壳（WKWebView）与浏览器内打开行为均无回归（壳内本就无浏览器 chrome，standalone meta 不应改变它）

**边界.** 不动对讲台 / 看板 / 详情的任何版面规则与决策 218④ 钉的常量；不做 iOS Fullscreen API 相关尝试（iPhone 不支持任意元素全屏）；「阅读模式」不在本票（见 spec.md out-of-scope）。

## 实现记录（2026-09-25）

- **图标直接复用桌面壳那一份**（实现中用户裁定）：`scripts/make-icon.mjs` 的 32×32 母版是全仓唯一图源（取色有 `--check` 对规格 §2.1 机器核对），本票**不另造第二套图形**——生成器加了一段 Web/PWA 出图：`192 = 32×6`、`512 = 32×16` 全整数倍，写 `frontend/public/icons/`；`any` 用桌面同款内缩圆角版，`maskable` 与 `apple-touch-icon` 用**满幅底**版（iOS 不认透明，圆角版的透明四角会露黑角；安卓遮罩裁的是 maskable）。180 非整数倍不出档，iOS 自己缩 192。
- **manifest**（`frontend/public/manifest.webmanifest`）：`display: standalone`、名称照 `<title>`、`theme_color` / `background_color` = 夜班靛 `#1b1d2c`。`start_url: "/"` 与配对令牌的关系：安卓 WebAPK 与 Chrome 共享 localStorage（先在浏览器里配对、再添加主屏即已配对）；iOS 靠图标记下的 `?pair=` 地址（决策 191 的既有机理）。
- **index.html**：viewport meta 追加 `interactive-widget=resizes-content`；manifest 链接、`theme-color`、`mobile-web-app-capable` + `apple-mobile-web-app-capable`、`apple-mobile-web-app-status-bar-style: black`、`apple-touch-icon`。status-bar-style 刻意取**实心黑**而非 `black-translucent`：仓里只有底部安全区基建（`--safeb`），黑透明会把顶栏压进刘海——那笔账不在本票。
- **指引文案**（对讲台两处配对说明）：补「从图标打开即是**独立窗口**（没有地址栏与底栏，决策 282 ④）」；「换过令牌后要重新添加一次（图标里记的是当时那条带令牌的地址）」照旧保留。
- **e2e**（`shell.spec.ts`）：meta 内容（含 `viewport-fit=cover` 与 `resizes-content`）、manifest 可取且声明 standalone、四枚图标 + apple-touch-icon 逐枚可取且非空——全绿。桌面壳（WKWebView）无回归由机理保证：壳内本无浏览器 chrome，这些 meta 在壳里惰性（注释随 index.html）。
- **两条访问路径**：静态产物由 axum 同一组路由托管，配对令牌层只拦写请求与 `/foreman/*`（决策 182㉖㉗㉘），`/manifest.webmanifest` 与 `/icons/*` 在 LAN 令牌路径下同样可读——e2e 跑的是 127.0.0.1 一条，另一条由该边界推理覆盖。
- **待真机目检三项**（e2e 不可达，人手过一遍）：iPhone 添加主屏后打开为 standalone（无地址栏/底栏）、输入坞不与 Home 指示条重叠；安卓 Chrome 聚焦输入框键盘压缩版面、坞与发送钮可见。
