# 04: 文档与威胁模型回填——把新接受的风险写在明处

**What to build:** 决策 206 修订了 19（修订）/ 104 / 179 的残余风险接受面，这一步把它落到文档。

- `docs/operations.md:1179` 那一行（表格里的「agent 通过 `run_command` 读取 `agentpipeline.db` 拿走
  provider 密钥」）今天只写流水线，改为「流水线 **+ 对讲台**（`ask` 档被拒，`auto` 档可达）」，
  并在同处标注「**无补偿**，根本解是 OS 级沙箱（同节末尾，`operations.md:1226`）」；
- `docs/glossary.md:33`（值班长）的「**不给文件系统读权限**」「它只说话、不动手」按新事实改写
  （域、档位、提议与确认钮）；新增「环境层 / 本服务写接口」「权限档位」两条词条；
- `docs/testing.md:137` 那条「值班长能力扩面是方向裁决、尚无实现」的提示改为指向本批票
  （`tests/foreman.rs` 的断言改写与 `talk.spec.ts` ⑦ 的改写各自归属哪张票要写清）；
- `design/theme-6-pixel.md` §3.3：确认钮**不新开一档「响」**（全站唯一告警仍是急停，决策 203），
  写清它与急停轮的关系；
- `design/frontend-design.md` §12.3 的行为表补行（决策 199 的机器校验：位置用反引号、
  备注含决策号），`frontend/src/lib/behavior-map.test.ts` 随之绿。

**Blocked by:** 01、02、03（要按落地后的实际界面写）

**Status:** done

- [x] operations 残余风险表那一行改写（含「无补偿」与指向沙箱的出口）
- [x] glossary 的值班长词条改写 + 新词条
- [x] testing.md 的提示改写（并把两条断言的归属写清）
- [x] theme-6-pixel §3.3 确认钮的档位一句
- [x] frontend-design §12.3 补行；`behavior-map.test.ts` 绿
- [x] 用户可见文案不含内部编号；`copy-discipline.test.ts` 绿

## 交付

- **`docs/operations.md` §12.14** 的残余风险表新增一行（值班长的命令面），并写明**无补偿**、
  指向 §12.15 末尾那句「OS 级沙箱是唯一根本解」；那一句也补了一句「这也是值班长那一条的出口」。
- **`docs/glossary.md`**：`值班长` 词条改写（只说话 → 能读、能提议，动手的是值班经理），
  新增 `环境层 / 本服务写接口` 与 `权限档位（env_mode）` 两条。
- **`docs/testing.md`**：「值班长能力扩面」那一段改成「三层都已落地」，并列出一张小表写清
  **两条断言的归属**（工具集断言 / `talk.spec.ts` ⑦ 各自钉什么），另附「状态区为空时零按钮」
  那条为什么保留。§6 / §7 / §9 的计数与端点行按落地后的实际数字改过。
- **`design/theme-6-pixel.md` §3.3**：表格新增「提议轮 `.turn.prop`」一行，纪律 2 补确认钮那段
  （**不新开一档「响」**、类名不叫 `.warn`、重复时只指路），纪律 4 补「提议过什么 / 你按了什么也进审计」。
- **`design/frontend-design.md` §12.3** 补四行，`behavior-map.test.ts` 46 条绿、
  `copy-discipline.test.ts` 8 条绿（新增文案里没有内部编号）。
