# 03: 全站像素控件与基元

**What to build:** 所有会被反复用到的界面零件变成像素钮与像素框：按钮、输入框、选择框、
状态标记、页签、焦点环、滚动条、toast。做完后随便打开一个对话框（如新建任务）或触发一条
提醒，就能看到主题已经成立——后续各页面只是把这些零件拼起来。

**Blocked by:** 02 主题契约模块 + token 层重写 + 字体自托管 + 视觉护栏起始

**Status:** ready-for-agent

- [ ] **按钮**：像素钮（2px 描边 + 3px 硬投影 + 按压位移消影）；主动作 = go 实心 + `▶` 前缀
      （实心钮前景用 `--go-ink`）；方括号装饰（`.btn::before/::after`）与 `--bracket` 一并移除；
      保留 `.solid` / `.quiet` / `.danger` / `.block` 的语义变体与其可访问名
      （**方括号只由 CSS 生成的约定要改**：可访问名不得含方括号，e2e 用 `getByRole('button', {name})`
      的调用点必须仍然匹配）
- [ ] **输入 / 选择 / 文本域**：`.input` 系列改 2px 描边像素框、圆角 0、`--input` 底色；
      焦点态是描边亮一档而非柔光；移动款维持 16px 字号防 iOS 聚焦缩放
- [ ] **状态标记**：`.st` 系列（run / wait / stop / dim）改为像素双编码——颜色 + 形状/文字，
      不靠色相单独承载；`.warn-dot` 呼吸改为离散帧步进
- [ ] **页签基元**：工位标签盒（active = wash 实底 + 描边上浮；禁用 = dither 底）做成可复用样式，
      供详情五页签与移动款页导航共用
- [ ] **焦点环**：`:focus-visible` 为 2px 硬描边、可见、不被 `outline-offset` 吃掉
- [ ] **滚动条**：窄条、方角、用 `--pane`；横向滚动隐藏工具类（`.no-scrollbar`）保留
- [ ] **toast**：`.toast` 改像素框 + 硬投影，`aria-live="polite"` 与关闭 `×` 的可访问名保留；
      pending 类仍带琥珀（全站唯一告警）
- [ ] **`.panel` / `.notice` 等共享原语**改像素框；`--r-panel` / `--r-pill` 恒为 0
- [ ] 动画只保留离散帧步进；`prefers-reduced-motion` 下静止
- [ ] 既有 vitest / playwright 全绿（本票预期不改断言语义，若有失败按「断言行为而非字形」修）
