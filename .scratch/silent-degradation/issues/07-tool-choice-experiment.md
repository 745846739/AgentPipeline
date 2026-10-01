# 07: 实验——`tool_choice` 强制原生工具调用（不挡任何一批）

**What to build:** 一个**可独立证伪**的实验，目标是**从源头消掉**票 01 要防的那一类截断。

背景：本模型的工具调用有两种落法——**原生** `tool_calls`（参数完整）与**文本**
`<tool_call><function=…><parameter=…></function></tool_call>` 落在 `content` 里
（此时网关另给一份结构化 `tool_calls`，而那一份会被腰斩）。事故里见过的失败形态全是第二种。
现在的请求体（`crates/core/src/agent/providers/openai.rs:38-59`）只发
`model` / `stream` / `stream_options` / `messages` / `temperature` / `max_tokens` / `tools`，
**没有 `tool_choice`**。

实验：对 agent 节点请求带上 `tool_choice`（先 `"auto"`，再试 `"required"`），
观察"文本形态占比"是否降到 ~0。判据取可观测的：一次真实任务跑下来，
模型响应 `content` 里出现 `<tool_call>` 的比例。

**这件事的定位是"上游可能自己就好了"**，所以它**不挡任何一批**，也不许写成任何修复的前提：
网关侧我们控制不了，实验失败就照票 01 的客户端防御走。

**Blocked by:** None

**Status:** ready-for-agent（2026-10-01；独立实验，与全部批次并行）

## 落点

- `crates/core/src/agent/providers/openai.rs:38-59` `build_body`：加 `tool_choice`（仅当
  `!request.tools.is_empty()`）。
- 是否可配置：先硬编码在实验分支上，**不**进 `Settings`——实验没结论前不给它设置项。
- 两个已知风险要在实现里处理：① 网关可能不认这个字段（400 / 报错，要能识别并回退到不发）；
  ② `"required"` 会强制模型每轮必须调工具，可能改变行为（例如本该长考的一轮被迫调工具），
  所以**先 `"auto"` 后 `"required"`**，分别记数。

## 验收

- [x] 同一份真实 prompt + 转录，各发 N 次（N ≥ 20）：记录 `content` 含 `<tool_call>` 的比例
      —— 不发 `tool_choice`（基线）/ `"auto"` / `"required"` 三组
- [ ] 若某档把比例压到 0 且不改变工具选择行为（同一输入选的工具与参数合理）→ 记结论"有效"
- [x] 若三档无差异或网关不支持 → 记结论"无效"，**关闭本票**，票 01 的客户端防御是最终答案
- [x] 结论（含原始计数）追加在本票 `## Answer` 下，并在 `spec.md` 的决议区补一行
- [x] 无论结论如何，**不许**把实验代码留在主路径上而不留开关/注释

**明确不做**：不为它加设置项；不改任何 prompt 去"教模型用原生工具调用"
（那是猜测网关行为，且会与票 03 的字段对齐工作打架）。

**来源：** `.scratch/silent-degradation/spec.md` 决议 2 的 (d)；诊断期已做过的小样本复现见
`spec.md` 缺陷 1（同 prompt + 转录重放 3 次返回的都是原生 `tool_calls`，说明触发是间歇的，
本票要测的正是"占比"而不是"能不能复现"）。

## Answer（2026-10-01）

**结论：无效——关闭本票，票 01 的客户端防御是最终答案。** 生产代码一个字没动（探针是
`.scratch/silent-degradation/tools/tool_choice_probe.py`，只读 DB 取 provider 配置、不写任何表）。

### 做法
用一次**真实调用**的 system + user + 转录当输入：`kanban_node_conversations` id 53
（architect-design.execute，system 3888 / user 240 字符 + 33 条转录，共约 60 KB），
按三档各发 20 次（模型 `xiaomi/mimo-v2.6-flash`，经 `https://api.commandcode.ai/provider/v1/`）。
判据：响应的 `message.content` 里是否出现 `<tool_call>`。

### 原始计数

| 组 | 文本形态 | 带原生 `tool_calls` | 失败 |
| --- | --- | --- | --- |
| baseline（不发 `tool_choice`） | **0 / 20** | 18 / 20 | 0 |
| `"auto"` | **0 / 20** | 19 / 20 | 0 |
| `"required"` | **0 / 20** | 16 / 20 | 0 |

（逐条明细与 `finish_reason` 见 `tools/tool-choice-counts.json`。）

### 读法（两条都写下来，免得把空结果读成"证明"）
1. **三档无差异**（0/20 对 0/20 对 0/20）——按票面的判据即"无效"。但要说清这份 0% 是
   **这一组输入没能复现**：诊断期同样的输入小样本重放也是原生 `tool_calls`（spec §缺陷 1 已记
   "触发是间歇的"）。故这个空结果**不足以证伪**"`tool_choice` 在某些输入下有用"，
   它足以支撑的是下面那条更强的证据。
2. **`"required"` 没被网关当回事**——它本该强制每轮都调工具，实测 20 次里有 **4 次**
   `native=0`（`finish_reason=stop`），比另外两组还少。这说明这个字段在网关那一侧要么被丢掉、
   要么不被严格实现。一个连 `required` 都不照做的开关，指望它压低"文本形态占比"是不成立的。
   → **不把它写进请求体**（生产至今没发过这个字段，维持原样）。

### 复跑命令（换模型 / 换输入时重来一次）
```
python3 .scratch/silent-degradation/tools/tool_choice_probe.py --n 20 --conversation-id 53
python3 .scratch/silent-degradation/tools/tool_choice_probe.py --n 20 --prompt-file /tmp/real.txt
```
一个实现细节值得留着：探针必须**显式发空 `User-Agent`**（生产的 reqwest 不发这个头，而
urllib 默认发 `Python-urllib/3.x`）——实测该网关前面的 Cloudflare 会据此回 403 / error code 1010。
