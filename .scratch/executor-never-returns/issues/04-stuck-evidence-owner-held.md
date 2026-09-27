# 04: 卡住判据补第三类——执行权持有超时（显式修订决策 210⑧ 判据面）

**Spec:** `../spec.md`（Implementation Decisions 第 4 条）

**What to build:** 作为值班经理，我要 `unstick` **认得出这次这个形态**：游标 `pending` + run 已终态 +
执行权仍持有。今天的判据两条都不覆盖——第一条被「游标必须可运行」先行挡掉，第二条要求 run 仍 `Running`。
结果是：兜底机制在它本该兜的那一格失效了，只能靠重启。

**Blocked by:** None (can start immediately)

**Status:** done（已实现，决策 302–311；四门 `make check` 全绿）

- [x] 第三类判据 = **执行权持有超过阈值**，**允许游标处于 `pending`**
- [x] **有无活 run 是同一条判据的两个分支，不拆成两条**——`unstick` 文件头明确警告过「报出来的卡住」与「解得开的卡住」必须是同一个集合，判据只能有一处实现
- [x] 三种构造形态都能被认出：run 仍 `Running` / run 已终态 / **尚无 run**（claim 了却还没建出 run）
- [x] **反向断言**：正常在跑的任务（心跳在走、未超阈值）不被误判
- [x] 认出之后 `unstick` **真的解得开**：清执行权 + 游标转 `pending` + 后续 `resume` 能跑
- [x] **显式修订决策 210⑧ 的判据面**并落号
- [x] 词汇表与 `docs/testing.md` 中「判据只认两类」的明文同步改
- [x] 四门通过
