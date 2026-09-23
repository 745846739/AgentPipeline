# 02: 成员表——`Stage` / `PendingKind` 的成员由 Rust 导出、前端断言（决策 253 落档）

**What to build:** 前端手抄了两个枚举的**成员集**：`Stage`（`frontend/src/api/types.ts:21-35`）与
`PendingKind`（`:37-45`）。两份**没有任何测试读 Rust**。本票让成员表机械地从 Rust 导出、前端断言
自己与它一致——前端不再手抄枚举。

**为什么这批走「Rust 导出」而不是「手写共享表」（决策 253 ②）：** `Stage` / `PendingKind` 的成员
**就是枚举本身**，手抄一遍等于把枚举抄第二遍，而抄的那份自己还会漂。权威表已经在 Rust 侧：
`crates/core/src/types.rs:33` 的 `ALL_STAGES: [Stage; 10]` 与 `:279-323` 的 `PendingKind` 枚举
加它的 `as_str` / `FromStr`。导出它即可，不必手写第三份。

**与票 03 的分界**：本票只管**成员**（有哪些值）；**顺序、伪键归属、展示标签**是规格，走票 03 的
手写表。两者分开是因为漂移症状不同——成员表漂了是「前端认不出一个值」（`FromStr` 失败、格子
不显示），规格表漂了是「格子顺序不对 / 伪键掉出配置页」，红了要能一眼分出是哪一类。

**Blocked by:** None（可立即开始）

**Status:** done（2026-09-23 实现）

- [x] 在 Rust 侧加一条导出路径，把两个枚举的成员落成一份 JSON 表到 `tests/fixtures/`
      （照决策 246 `tests/fixtures/host_policy_loopback.json` 与决策 250 `tests/fixtures/repo_id.json`
      的先例；路径与形态与票 03 约定一致）。**推荐形态**：`tests/fixtures/enum_members.json`，
      形如 `{"stage": [...], "pending_kind": [...]}`，每个数组**按键的声明顺序**排列——
      顺序本身是票 03 的规格，但导出时顺手带上，两份表不冲突（票 03 断言的是「前端格子顺序
      与它一致」，本票断言的是「集合相等」）
- [x] **Rust 侧**：加一条测试断言导出结果与 `ALL_STAGES` / `PendingKind::as_str` 全量一致——
      即「导出没有漏一个变体」。**判据必须遍历枚举**，不能手写数组（手写就又是抄一遍）。
      `ALL_STAGES` 已是 `[Stage; 10]` 数组、`PendingKind` 需要一条穷举 match 或 `FromStr` 往返
      （`PendingKind` 的 `as_str` / `FromStr` 已是权威表，用它）
- [x] **前端侧**：加一条 vitest 读同一份 fixture，断言 `Stage` / `PendingKind` 两个 TS 联合的
      成员集与之**集合相等**（不是「包含」——多一个值也要红，那正是「前端认得出后端认不出的值」
      这一类漂移）
- [x] 用 `@vitest-environment node` 与 `import.meta.url` 定位仓库根（照
      `frontend/src/lib/hostPolicyFixture.test.ts` 与 `marketReposFixture.test.ts` 的文件头注：
      默认 jsdom 下 `import.meta.url` 是 http 形态、`fileURLToPath` 会抛）
- [x] 表内**必需行按名钉住**（照 `marketReposFixture.test.ts` 的做法：只数行数时，随便塞行多余
      输入也能过）——至少钉 `'done'`（既是 Stage 又是终态）、`'foreman'`（**不是** Stage 成员、
      只在配置键里）、`'merge_approval'`、`'user_decision'`
- [x] `docs/testing.md` 用例目录加 row（L1 段，锚决策 253）
- [x] `docs/decisions.md` 追加**决策 253**（本批已落，核对即可）；
      `AGENTS.md:5` 与 `docs/README.md:18` 的计数改到 `#1–254`
- [x] `make check` 绿

**零行为变化**（纯加表与测试）。

**明确不做**：不改 `api/types.ts` 两个联合的**内容**（本票只给它们加一条断言；若断言发现它们
今天就对不上，那是本票要暴露的第一个发现，**就地记在票面**而不是顺手改到绿）；不管顺序与伪键
（票 03）；不给 `TransitionTrigger` / `CursorStatus` / `TaskStatus` 等其余联合做表（本票只做
决策 253 点名的两个；其余按同一形状可续，另立票）。

## 实现者记事（2026-09-23）

**断言发现它们今天对得上**——两个联合与枚举逐值一致，零暴露（本票的第一个可能发现没有出现）。

**导出不经过手写数组，改用 `schemars` 宏**。票面写的是「判据必须遍历枚举，不能手写数组
（手写就又是抄一遍）」，而票面给的落点是「`ALL_STAGES` 已是 `[Stage; 10]` 数组、`PendingKind`
用它自己的 `as_str` / `FromStr` 往返」。落地时发现**那条路仍留着一个洞**：`ALL_STAGES` 是
手写数组，枚举新增一个变体而忘了加进它，导出照样漏——只是把「抄枚举」换成了「抄数组」。
两个枚举都已 `derive(JsonSchema)`，故改用 `schemars::schema_for!(Stage)`：**变体表由宏从
枚举定义取**，手写清单位置为零。实测两种输出形状（无文档注释的枚举给 `enum`，带文档注释的
变体走 `oneOf`），导出函数两种都收，并展开同组多值（`TaskStatus` 的
`done`/`failed`/`cancelled` 就是这样并成一个 `enum` 的——只取 `[0]` 会漏两个，落地时已修正）。

**`tests/fixtures/enum_members.json` 是生成物，不是手写表**（与票 03 那份规格表相反）。票面
没有明说这一点，但按「成员表机械、规格表手写」的分界，它只能是导出物——文件头注写明了，
表测试失败时报文带**可直接贴回的 JSON**（照 `scripts/make-icon.mjs --check` 的姿态：
不手改生成物，重跑生成）。

**前端两个联合改成从成员表推导**（`export const STAGE_MEMBERS = [...] as const;` +
`export type Stage = (typeof STAGE_MEMBERS)[number]`）。票面只要求「加一条断言」，但若断言
断的是一个**新加的数组**而联合仍是另写的一串字面量，那等于又添了第三份副本、而断言看不住
联合本身。推导之后两者在**类型层**就不可能漂，fixture 断言只需管「成员表 ⟺ Rust 枚举」。
这是零行为变化：推导出来的联合与原来逐字相同（`svelte-check` 0 error 已验证）。
