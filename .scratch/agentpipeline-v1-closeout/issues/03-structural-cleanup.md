# 03: 结构性清理批次

**What to build:** 清理三处代码评审记录的「结构性待清理」（非行为缺陷），让后续改动的落点唯一：merge 的 rebase 自动合并逻辑搬回 git 层；前端 action→endpoint 映射去掉第二份手写副本；judge continue 的落点逻辑复用既有的游标推进函数而非复刻。行为零变化，纯搬移与去重。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] rebase 自动合并逻辑住在 git 层，执行器不再含该 git 细节；可自动合并 / 硬冲突的行为与用例不变
- [x] 前端 action→endpoint 映射有单一事实来源，另一处改为引用
- [x] `advance_after_judge_continue` 复用既有游标落点逻辑，不再复刻
- [x] 全部质量闸门绿：fmt / clippy / `cargo test --workspace` / 前端 vitest + svelte-check + build
- [ ] 该票**不夹带行为变更**：任何顺带发现的行为问题另立票，不在本票修——**无法判定**：这是对当时
      那次提交的过程性负向断言，其后仓库又叠了决策 158–208 等多轮改动，今日无从核对当时是否夹带；
      票面也无叙述。2026-09-17 回填时按此不勾
