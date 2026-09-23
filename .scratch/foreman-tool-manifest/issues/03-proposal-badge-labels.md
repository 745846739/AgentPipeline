# 03: 提议徽章基词同源——`proposalToolLabel` 收 `labels` 参数

**What to build:** 提议徽章与回执说**同一个词**——`proposalToolLabel` 加 `labels` 参数（02 号票的取数缓存传入），8 个 case 里的基词删掉、改从后端 label 取；`· action` 后缀与特例留前端：`task` / `config` / `skills` 拼 `· action`，`service` 无后缀，`repair` 固定句**「修复 · 合入分支」**不变（决策 212 的措辞，「合入」是按下前最该看见的字），认不出的工具名（`pairing` 那类）原样显示——那条既有测试不动。唯一可见变化：skills 提议徽章从 `技能 · install` 统一成 `技能动作 · install`——两份镜像今天已经打架（`技能 · install` vs `技能动作`），统一必动其中一面，动的是按 02 新表走的这一面。单测改为传字面量 labels，不依赖网络。

**Blocked by:** 02（要吃 02 号票建立的取数缓存）

**Status:** done（2026-09-23）

- [x] `proposalToolLabel` 签名加 `labels` 参数：基词查 `labels[tool]`，查不到回落 `tool` 原样（与 `labelFor` 同一兜底语义）；8 个 case 的基词字面量删除
- [x] 后缀规则留前端且不动：`task` / `config` / `skills` 拼 `· action`，`service` 保持无后缀，`repair` 返回固定句「修复 · 合入分支」（不查 label）
- [x] `pairing` 原样显示的既有用例不改仍绿
- [x] 唯一可见变化落测：skills 提议徽章断言从 `技能 · install` 改为 `技能动作 · install`（其余徽章文案断言不变——基词本就与现表逐字一致）
- [x] `proposalToolLabel` 单测全部改为传字面量 labels（不发请求）；Talk 调用点传入 02 的缓存
- [x] `make check` 绿
