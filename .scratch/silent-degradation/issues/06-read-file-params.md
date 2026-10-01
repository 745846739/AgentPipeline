# 06: `read_file` 参数姿态（强转数字字符串 + 禁止静默降级）

**What to build:** 让 `read_file` 的分页**真的能用**，并且参数错型时**响**而不是静默兜底。

现状（`crates/core/src/agent/tools.rs:1115-1133`）：

```rust
let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
let limit  = args.get("limit").and_then(|v| v.as_u64()).map(|v| v as usize);
```

模型把参数写成**字符串**（事故里实际是 `{"path": "…", "offset": "1", "limit": "90"}`），
`Value::as_u64()` 对 `"1"` 返回 `None` → `offset=0, limit=None` → **静默整份读** →
超过 `offload_threshold_tokens`（默认 4000）→ 卸载 → 模型以为没读到 → 再读 → 再卸载。
同一份 design.md 在 validate_output 那一轮被连读 7 次、在 architect execute 那一轮被读 8 次；
任务目录里堆了 **84 个 `.context/*.txt` / 1.6MB**。

「静默兜底成整份读」是这里最坏的选择：它既没满足分页意图，又正好把结果推过卸载阈值，
还让模型无法从结果里看出参数被忽略了。分页本来能救——90 行远低于 4000 token。

改法（决议 Q6）：
① 数字字符串强转（`"90"` → 90），模型把数字写成字符串很常见；
② 参数**未知或错型**时**报错**，不许静默降级成"读整份"——错误文案要能让模型自纠
（说明该参数期望什么类型）；
③ 顺带核对同族的宽松读取：`list_dir` 的 `recursive`（`as_bool`）、
`run_command` / 其他工具的同类字段，同一姿态一次改齐。

> 注：卸载提示语（`crates/core/src/agent/context.rs:295`）教模型「用 read_file("{path}")
> 读完整内容」，而那个 `.context` 文件本身也超阈值 → 卸载的卸载。
> 那条豁免在**票 01 的 ④**里，不在本票；本票只管参数。

**Blocked by:** None（与票 01 无代码依赖，只是主题相邻）

**Status:** ready-for-agent（2026-10-01；批次二）

## 落点

- `crates/core/src/agent/tools.rs:1115-1133` `read_file` 的参数解析。
- 建议抽一个小助手（如 `arg_usize(args, "limit")` / `arg_bool(args, "recursive")`），
  放在同文件参数解析区，**让"强转 + 报错"口径只写一份**，而不是每个工具各写一遍。
- `crates/core/src/agent/tools.rs:955` 附近是 L2 卸载策略的说明，不是本票的落点（对照即可）。

## 验收

- [x] 单测：`read_file {"path": "design.md", "offset": "1", "limit": "90"}` 返回**恰好 90 行**
      （不是整份、也不触发 L2 卸载）
- [x] 单测：`"limit": "abc"` → **报错**，文案指明 `limit` 期望整数；不许返回整份文件
- [x] 单测：`"limit": 90`（数字）与 `"limit": "90"`（字符串）结果**逐字节相同**
- [x] 单测：未知参数名不静默吞（若有既有工具依赖"未知键忽略"的语义，明确记在本票验收里，
      不要顺手改了别的工具的行为）
      ——**未知键仍按既有语义忽略**（本票只改"已知键的形态错了怎么办"），记在此处、
      不改别的工具
- [ ] 用当前事故现场真跑一次：同一条 `read_file` 从"整份卸载"变成"90 行正文"，
      且不再产生新的 `.context` 文件
      （**本地做不到**：那要在 106 上跑真任务。等价单测见下——同一条调用在同一接缝上从
      "整份"变"恰好 90 行"）
- [ ] `make check` 全绿（本地按决策 331 跑 `make check-lint` + 改动层的窄跑，全量在 CI）

**明确不做**：不改 `offload_threshold_tokens` 默认值；不改 `trim_read_file` 的 L1 裁剪口径；
不做"自动分页"（一次调用返回全部内容）——那是另一个设计问题。

**来源：** `.scratch/silent-degradation/spec.md` 放大器二；现场证据
`kanban_node_conversations` id 99（7 次连续卸载，每份 4657 token）与 id 102（8 次）。

## 落地记录（2026-10-01）

- **两个小助手**（`crates/core/src/agent/tools.rs` 的参数解析区，口径只写一份）：
  - `arg_u64(args, key) -> Result<Option<u64>>`：数字直取，数字字符串 trim 后 parse；
    其余形态（`"abc"` / 对象 / 负数）**报错**，文案写明"期望一个非负整数（可以写成数字或
    数字字符串）"。
  - `arg_bool(args, key) -> Result<Option<bool>>`：`true/false` 与字符串 `"true"/"false"`；
    其余**报错**。缺键与 `null` 都按"没给"处理（保持各工具的既有默认值语义）。
- **同族一次改齐**（票面 ③）：
  - `read_file`：`offset` / `limit` / `tail`；
  - `list_dir`：`recursive`；
  - `read_board`：`runs`；`read_conversation`：`run_id`；
  - `run_command` / `run_command_argv` / `web_fetch`：`timeout_sec`。
  这些此前都是 `and_then(as_u64 / as_bool).unwrap_or(默认)`——同一个病：模型写错形态时
  **静默退回默认值**，调用意图落空且不可见；`timeout_sec` 被写坏尤其危险（悄悄用阶段默认值）。
- **测试**：`read_file_coerces_numeric_strings_instead_of_reading_everything`
  （200 行文件 + `{"offset":"1","limit":"90"}` → 恰好 90 行，且与数字写法逐字节相同）、
  `read_file_rejects_a_limit_that_is_not_a_number`（报文含 `limit` 与"非负整数"）、
  `list_dir_coerces_string_booleans_and_rejects_other_shapes`。
- 明确不做（票面）：不动 `offload_threshold_tokens`、不动 `trim_read_file` 的 L1 口径、
  不做自动分页。
