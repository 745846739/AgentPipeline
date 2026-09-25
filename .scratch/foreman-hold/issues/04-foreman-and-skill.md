# 04: 值班长的能力面与操作手册

**What to build:** 值班长在 `task` 工具上拿到 `pause` / `rerun` 两个动作（与界面上那两颗
**同形**），`operate-pipeline` 手册从「七个动作」改成九个并补一节讲清三颗钮的分工。
裁决落 `docs/decisions.md` 276 的 ⑦。

**Blocked by:** 01（端点）与 03（界面上那两颗钮先钉住「同形」的形状）

**Status:** done（2026-09-25）

**要点：**

- **不发明第二套参数语言**（票 foreman-actions 05 的硬要求）：两个动作直接调
  `tasks::pause` / `tasks::rerun` 的 handler，报错文案与界面上那颗钮看到的是同一句。
- 手册只做翻译：三颗钮的**分步纪律**（暂停 → 续跑/重跑各是一张卡，别指望一次按键做完两件）、
  「已经停着就别用 rerun」「本阶段没跑过会被拒」「想重跑整条用 retry 且只对终态」都写在
  手册里；托管不替他松开这条写成**系统事实**（不是要它记住的规矩）。

- [x] `crates/core/src/pipeline/foreman.rs`：`task` 工具的描述与 `enum` 加两个动作
- [x] `crates/app/src/routes/foreman.rs`：分派两臂 + 「可用：」那句同步
- [x] `crates/core/src/agent/factory/operate-pipeline/SKILL.md`：范围声明的九个动作、写流程
      第 2 条的逐条适用（含 `resume` 那条「人按住过的任务也从这里松开」）、新一节
      「非 pending 任务的三颗钮」
- [x] 用例：§7 `api_contract.rs::the_foreman_can_pause_a_task_once_its_proposal_is_pressed`
      （提议 → 按键 → 真的按住；分派漏一条臂的回归）
