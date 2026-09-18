# 11: 校验缺口：base_url 形状、拆分空标题、静默 no-op

**叠:** A（不动规格）

**来源:** R2-13（base_url 实测；拆分部分代码）

**What to build:**

**① `base_url` 非法值直接落库（实测）.** 填 `not a url` → 创建成功、零字段报错、列表逐字显示：
```
{"fieldErrors":[], "rowsWithBadUrl":["openai gpt-4o-mini [ON] ctx 128,000 base_url not a url …"]}
```
`validateProviderDraft`（`lib/providers.ts:63-70`）只校验 vendor / model / context_window，
后端也直接落库。后果延后到任务真跑模型时才炸，离填写现场很远。

**② 拆分行能造出空标题任务.** `SplitDialog.svelte:18-26` 的 `line.split('|')` 让 `| 说明`
产出 `{title:'', description:'说明'}`——原任务被取消、一个**空标题**子任务被创建。

**③ 静默 no-op.** 文本域全空时点「确认拆分」什么都不发生、也不说为什么
（`if (tasks.length > 0)`），而现有单测 `SplitDialog.test.ts:41-51` 正好把这个行为钉住了。

**Blocked by:** None（can start immediately）

**Status:** open

- [ ] `base_url` 非空时校验形状（协议 + 主机），不合法给**字段级**错误并拦住提交
- [ ] 拆分：标题为空的行拦下并指出**第几行**；不产生空标题子任务
- [ ] 拆分：文本域全空时给提示（或在按钮上体现不可提交），不做静默 no-op
- [ ] 单测：`validateProviderDraft` 对 `not a url` / `ftp://x` / 空值 各自的行为
- [ ] 单测：`SplitDialog` 空标题行、全空文本域的行为（**随之改写 `SplitDialog.test.ts:41-51` 那条**）
- [ ] e2e：provider 表单填非法 base_url → 断言被拦下且未创建
- [ ] e2e：拆分表单放一行 `| 说明` → 断言被拦下并指出行号

**边界.** 后端要不要一并补校验另议（本轮只要求前端当场拦下，不让脏值落库）。
