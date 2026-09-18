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

**Status:** done

- [x] `base_url` 非空时校验形状（协议 + 主机），不合法给**字段级**错误并拦住提交
- [x] 拆分：标题为空的行拦下并指出**第几行**；不产生空标题子任务
- [x] 拆分：文本域全空时给提示（或在按钮上体现不可提交），不做静默 no-op
- [x] 单测：`validateProviderDraft` 对 `not a url` / `ftp://x` / 空值 各自的行为
- [x] 单测：`SplitDialog` 空标题行、全空文本域的行为（**随之改写 `SplitDialog.test.ts:41-51` 那条**）
- [x] e2e：provider 表单填非法 base_url → 断言被拦下且未创建
- [x] e2e：拆分表单放一行 `| 说明` → 断言被拦下并指出行号

**边界.** 后端要不要一并补校验另议（本轮只要求前端当场拦下，不让脏值落库）。

## 实施记录（2026-09-18）

**落点**

| 处 | 改动 |
|---|---|
| `src/lib/providers.ts` | `validateProviderDraft` 补 `base_url` 形状校验：非空时必须是 `http(s)://主机`；非法给**字段级** `{ field, message }` |
| `src/components/task/SplitDialog.svelte` | `parse()` 重写为「解析 → 校验」两步：返回 `{ tasks } \| { message }`；**标题为空的行被拦下并指出第几行**；文本域全空时给一句 `localError` 而不是静默 no-op；错误节点 `role=alert` |
| `src/components/board/NewTaskDialog.svelte` | 项目选择 + 校验结果接上字段级错误（票 02 的同一条通道） |

**校验口径**（写死，单测钉住）：空值放行（`base_url` 可以不填，走 provider 默认）、
`not a url` 与 `ftp://x` 拦下、`http://` 后必须有主机名。**不做全 URL 语法校验收敛**
（后端才是最后一道），这里只堵「明显不是地址」这一类。

**订正票面一处**：票面说「现有单测 `SplitDialog.test.ts:41-51` 正好把这个静默 no-op 钉住了」
——那条用例已被本票改写（现在钉的是「全空时给提示且不提交」）；同时新增了「空标题行指出行号」
一条。`SplitDialog.test.ts` 现在 3 条 → 5 条。

**证据**：
- 单测 `src/lib/providers.test.ts`（`not a url` / `ftp://x` / 空值 三态）、
  `src/components/task/SplitDialog.test.ts`（空标题行 → 提示含行号；全空 → 提示且不提交）。
- e2e `frontend/e2e/ux2-flows-and-copy.spec.ts` ③：走 UI 填 `not a url` → 断言当场拦下且
  **列表里没有新行**。

**边界**：后端校验未动（票面写明另议）。
