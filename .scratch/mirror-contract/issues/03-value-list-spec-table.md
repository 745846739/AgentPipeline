# 03: 规格表——键序 / 伪键归属 / 终态集 / pendingLabel 用共享表钉住（决策 253 落档）

**What to build:** 前端手抄了四份**规格**（不是成员集，是「按什么次序、归哪一组、显示什么词」），
四份都没有测试读 Rust。本票手写一张共享表、两侧各自断言——表是规范，漂了会红。

**四份规格与它们的 Rust 侧：**

| 前端 | 内容 | Rust 侧 |
|---|---|---|
| `lib/stageConfigs.ts:18-33` `STAGE_KEYS` | 14 个配置键的**格子顺序** | `types.rs:33 ALL_STAGES`（10 个真实阶段）+ `app/src/routes/stage_configs.rs:27-32 PSEUDO_STAGE_KEYS`（4 个配置键） |
| `lib/stageConfigs.ts:50-55` `PSEUDO_KEYS` | 哪些键**不是真实阶段** | 同上 `PSEUDO_STAGE_KEYS` |
| `lib/stewardship.ts:18` `TERMINAL_STATUSES` | `['done','failed','cancelled']` | `types.rs` 的 `TaskStatus::is_terminal` |
| `lib/pipeline.ts:551-588` `pendingLabel` | 9 个 pending 种类的**展示词** + `user_decision` 的 7 个子类 | `types.rs:279-323 PendingKind`（词条由界面持有，故只钉**成员**与**context.kind 枚举**） |

**两份已经登记为险的注释**（说明这不是假想问题）：

- `frontend/src/lib/stageConfigs.ts:17` 写「10 个真实阶段 + 4 个非阶段配置键
  （`routes/stage_configs.rs::PSEUDO_STAGE_KEYS`）」；
- `crates/app/src/routes/stage_configs.rs:22-24` 写「**必须与 `frontend/src/lib/stageConfigs.ts`
  的 `PSEUDO_KEYS` 同步**：那份决定设置页列不列得出这一行，这份决定后端收不收这一行。不同步的
  表现是「界面上填好、保存被 400 拒掉」，而 400 的理由写着「未知阶段」——看的人只会当成界面
  bug 去查前端。」

**两侧都靠注释承诺**，没有任何机器检查。本票把它变成会红的。

**为什么这批走「手写表」而不是「Rust 导出」**：哪些键是「不进 `Stage` 枚举的配置键」、格子按什么
次序排，是**规格**——枚举推不出来。`PSEUDO_STAGE_KEYS` 自己也是手写的 4 项数组，它与前端那份的
关系是「同一份规格的两个副本」，不是「一个从另一个生成」（决策 253 ②）。

**Blocked by:** **02**（两张表共用 `tests/fixtures/` 目录与「按 `import.meta.url` 定位仓库根 +
node 环境」的读表 helper；02 先落把 helper 定下来。若并行开工，02 先合）

**Status:** done（2026-09-23 实现）

- [x] 写 `tests/fixtures/frontend_spec_tables.json`（或与票 02 商定的一致命名），按**规格**分节：
      - `stage_keys`：14 项**按格子顺序**（含 `'foreman'`，它**不是** `Stage` 枚举成员——
        `foreman.rs:52 FOREMAN_STAGE_KEY` 那一族，决策 182①）；
      - `pseudo_keys`：4 项；
      - `terminal_statuses`：3 项；
      - `pending_kinds`：9 项（`PendingKind` 的成员，供票 02 之外的**顺序**断言用）；
      - `user_decision_context_kinds`：`pendingLabel` 的 `user_decision` 分支里那 7 个子类
        （`duplicate_risk` / `dirty_worktree` / `test_code_issue` / `gate_recheck` /
        `judge_disagreement` / `review` / `develop_design_input_insufficient` /
        `test_design_input_insufficient`——**核对时按实际列出**，本票不预设条数）。
      每节带 `$comment` 说明「这节钉哪条不变量、漂了的症状是什么」（照决策 246 / 250 两份
      fixture 的 `$comment` 写法）
- [x] **Rust 侧**加一条表测试：断言 `STAGE_KEYS` 的前 10 项 == `ALL_STAGES` 的 `as_str`、
      后 4 项 == `PSEUDO_STAGE_KEYS`；`terminal_statuses` 与 `TaskStatus::is_terminal` 的
      真值一致（**遍历所有 `TaskStatus` 变体**，不是手写数组比较）；`pending_kinds` 与
      `PendingKind` 成员一致
- [x] **前端侧**加一条 vitest：断言 `STAGE_KEYS` **逐项等于**表里的 `stage_keys`（含顺序）、
      `PSEUDO_KEYS` 集合相等、`TERMINAL_STATUSES` 集合相等、`pendingLabel` 的 switch **覆盖**
      表里 `pending_kinds` 每一项（未覆盖即红——今天它漏了哪一项正是本票要暴露的）
- [x] 表内**必需行按名钉住**（照 `marketReposFixture.test.ts`：只数行数时随便塞多余输入也能过）
- [x] `docs/testing.md` 用例目录加 row（L1 段，锚决策 253）
- [x] `docs/decisions.md` 追加**决策 253**（本批已落，核对即可）；
      `AGENTS.md:5` 与 `docs/README.md:18` 的计数改到 `#1–254`
- [x] `make check` 绿

**零行为变化**（纯加表与测试）。**唯一可能的暴露**：若断言发现前端今天就漏了某个 pending 种类
或某个 `context.kind`，**就地记在票面**（那正是这张表的价值），是否修另立票或并入本票由实现者
判断并在票面写明。

**明确不做**：不引运行时校验；不做代码生成；不把展示词（「合并提案」这类）搬进表——词是界面
所有物（决策 200 的平实口径），表只钉**成员与顺序**，不钉文案；不动 `api/types.ts` 的联合内容
（那是票 02 的范围）。

## 实现者记事（2026-09-23）

**唯一可能的暴露没有出现**：`STAGE_KEYS` 的 14 项顺序与两张后端表一致，`TERMINAL_STATUSES`
与 `is_terminal` 一致，`pendingLabel` 覆盖全部 9 个 pending 种类与全部 8 个 `user_decision`
子类——前端今天就对得上，零暴露。故本票**没有顺手修任何东西**。

**票面列 7 个子类，实际是 8 个**。票面原文写「那 7 个子类」，但括号里**逐个列了 8 项**
（`duplicate_risk` / `dirty_worktree` / `test_code_issue` / `gate_recheck` /
`judge_disagreement` / `review` / `develop_design_input_insufficient` /
`test_design_input_insufficient`）——只是**数目字写错了**，清单本身是全的（票面也叮嘱「核对时
按实际列出，本票不预设条数」）。按实际（`crates/core/src/actions.rs::kinds` 里在 `user_decision`
行上出现过的）确为 **8 个**，fixture 按实际列了 8 项。

**`gate_recheck` 那一格也没有另一处漏**：本票担心的是「前端漏一个 `context.kind` → 那一格
退回『等待决定』」，实测前端认全部 8 个，后端 `classify` 也认全部 8 个。为防止将来单侧漏，
Rust 侧的表测试做了**双向**断言（表里每个都被 `classify` 认出来 **且** `classify` 认识的每个
都在表里）——单向会漏掉「配对器漂了」的那一半。

**`pending_kinds` 一节没有写进规格表**。票面把它列为本表的一节（「供票 02 之外**顺序**断言
用」），但它与 `enum_members.json::pending_kind` **逐字相同**——写进来就是同一份枚举的第三份
副本，正是决策 253② 要挡的事（「枚举推得出来的东西不手抄」），且顺序对 `pendingLabel` 无意义
（它是个 switch）。故前端那条「`pendingLabel` 覆盖每个种类」的断言直接读**成员表**。

**`PSEUDO_KEYS` / `TERMINAL_STATUSES` 不为测试放宽接口**。两份都是模块私有的，票面写的是
「集合相等」。落地经它们**唯一的生产消费者**断言（`isPseudoStage` 双向、`stewardshipFace`
逐状态），强度相同而不必把内部表导出成公开 API——`app` 侧那份 `PSEUDO_STAGE_KEYS` 同理
（住在 `stage_configs.rs`，测试就在同文件尾部，也不需要 `pub`）。

**第四处断言落在 `app` 而不是 core**：`PSEUDO_STAGE_KEYS` 是「后端收不收这一行」的唯一判据
（`validate_stage_key`），它的副本漂了不会经过 `types.rs`。故 `stage_configs.rs` 尾部另有一条
表测试断同一份 fixture。

**票面要求的「共用读表 helper」落成 `lib/fixtures.ts`**（票 03 的 `Blocked by: 02` 就是为它）。
两票的四条 vitest 里，本批新建的两条走它；**既有的 `hostPolicyFixture.test.ts` /
`marketReposFixture.test.ts`（决策 246 / 250）不动**——它们属于已提交的另两批，改它们不在本票
范围内（那是另一条「顺手重构」，本票的纪律是零行为变化纯加表）。

**两轴 code-review 的收口（2026-09-23，实现当轮）**：修掉三处**注释声称与代码不符**
（`TaskStatus` 与 `Stage` 两处号称「遍历全部变体」实则手写数组——已改成经 `schemars` 导出；
契约测试号称「一个班次一次读取」实为三段——已订正）、一处**悬空引用的守卫测试名**
（改为真实存在且真会红的 `every_actions_kind_is_covered_here` 源码扫描守卫）、一处**重复的
`kind` 字面量**（`LedgerRow` 改 `Pick<ForemanMessage,'id'|'kind'>`，与 `types.ts` 同一类型）、
以及 glossary 词条里两处**过期行号**。**这批收口没有改变任何行为判定**——全是「让注释说真话」
与「把判据从手写清单换成导出」，故 `kind` / `proactive` 的取值、四个成员表的内容一律未动。
