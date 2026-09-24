# 01: 内容搜索——只读层的 grep/rg 等价物

**What to build:**（先裁决、后动手——形状见裁决问）值班长深挖时要在文件域里**找内容**：
报错串、配置键、函数名落在哪几处。现状只读层没有任何检索手段——`list_dir` + `read_file`
逐个翻是唯一找法，triage 判词「**今天每天疼**」（决策 262①）。

**接缝事实（探查取证，2026-09-24）**：
- `READONLY_COMMANDS = ["date","ps","pgrep","lsof","wc","tail","sample"]`（`tools.rs:2707`，
  白名单有测试钉死）——**没有 grep/rg**；
- 工作区已带 `regex` crate（workspace `Cargo.toml:23`，core 已引）——纯 Rust 检索零新依赖；
- 清单条目形状 = `ForemanToolSpec{name,label,description,parameters}` 四字段
  （`pipeline/foreman.rs:77-87`），冻结契约现 23 项（`[ForemanToolSpec; 23]`，名字+顺序钉死）；
- 只读工具先例（237 的 `run_readonly`）：**两段名单都不进**——`ENV_TOOLS`（档位管的是
  「能不能改东西」，它改不了）与 `FOREMAN_WATCH_TOOL_DENY`（只读取证值守轮也该有）。

**分支草案（拷问输入，不是结论）**：

- **(a) 新只读工具（如 `search_content`），纯 Rust 实现**：参数 = 模式 + 起始路径（域内），
  行走、正则、命中截断、台账全自管；清单 23→24，广告里直接说「找内容用我」——
  262① 的判词「B 层姿势**进清单**」字面即此；测试不依赖系统二进制。
- **(b) `READONLY_COMMANDS` 白名单加 `grep`/`rg`**：改动最小，域校验 / 台账 / 输出
  三层截断（offload + 台账 preview + 12k transcript）全部白蹭；但模式串会走
  `readonly_path_arg` 的「非选项参数按路径校验」语义（凑合能用、不精确），且依赖系统
  二进制与 BSD/GNU 旗标差异。

**边界草案（拷问输入）**：域 = `home.root()` + `data/` 拒（206 同一条 `check_read`）；
命中输出带上限（行数与字符双 cap，transcript 口径 12k 沿用）；台账留行（只读也要看得见
尝试，179 姿态）；值守轮**可用**、不受 `env_mode` 管（照 `run_readonly` 先例，两项默认）。

**Blocked by:** None（裁决落 `docs/decisions.md` 后实现）

**Status:** done（2026-09-24）

- [x] 形状裁决取 **(a) 新只读工具、纯 Rust**（四问，决策 267）；值守轮可用、档位豁免（照 `run_readonly` 两项先例）
- [x] 实现 + 用例落齐（决策 267）：`FOREMAN_TOOL_SPECS` 23→24（`search_content` 插 `run_readonly` 之后）、
      `regex` + `std::fs` 行走（不跟 symlink / 二进制跳过 / 三上限 / 逐条过 `check_read`）、
      台账两态（域拒走 `refuse_readonly` 口径、坏正则 `Validation` 不落行）；
      断言：L2 新文件 `tests/integration/search.rs` **8 条** + 冻结契约 23→24 + label 23→24
