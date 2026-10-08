# 02: 子代理只读集加 `search_content`

**What to build:** 只读子代理（`SUB_AGENT_TOOLS`）从两件（`read_file` / `list_dir`）变成
三件：加 `search_content`——它今天只服务值班长，而**检索**正是子代理存在的理由。

**为什么**：2026-10-08 三个子代理全打满 12 轮，根因是「父代理派了检索型子任务，
子代理手里没有检索工具、也没有 shell」——只能一个个列目录、一个个读文件。子代理的
定位本来就是「把读 20 个文件的原文挡在父上下文之外」，把最省上下文的那件工具（正则
搜内容）关在门外是把它的本职活干不成。

**形状**：

- **一份 schema、两处消费**（显式修订决策 353 的「只此一份」为「一个定义、多处消费」）：
  `pattern` / `path` 的参数 schema 提成 `agent::catalog` 里的常量，值班长 spec 与子代理
  的 def **都从它取**；**描述各自持有**——值班长那份写的是值班长的域（「在你的文件域
  里」「data/ 读不到」），子代理写工作区版。
- **不进 builtin 面**（决策 267④ 的边界不动）：`search_content` 不进 `TOOL_SPECS` /
  `BUILTIN_TOOLS` / `ENV_TOOLS`，管线阶段仍**不能声明**它；名字常量照决策 353 住目录表。
- **deny 档下同收**：子代理的档位语义是「环境层收到底」（`subagent.rs` 的既有注释），
  而 `search_content` 不在 `ENV_TOOLS`——不额外过滤就会出现「deny 档下读不了文件、
  却能把文件内容搜出来」的洞。故 **deny 档下把它从白名单与广告里一并摘掉**。
- 执行点无需改动：`tools.rs` 的分发已按名字接好，白名单（`with_allowed_tools`）就是边界。

**Blocked by:** None

**Status:** done

- [x] 用例：子代理工具集恰为 `read_file` / `list_dir` / `search_content`（改既有安全断言
  `executor.rs::subagent_tool_set_is_read_only`，并加断言「广告非空壳、schema 要求 pattern」）
- [x] 用例：deny 档下子代理既拿不到广告、执行点也拒（`subagent.rs::deny_mode_takes_the_search_tool_away_from_the_subagent`
  ——判定收在 `effective_tools` 一处，广告与白名单都从它出；如实记：deny 下父节点本就派不出子代理，属纵深防御）
- [x] 用例：子代理调 `search_content` 在工作区里命中（`executor.rs::subagent_can_search_the_worktree_with_search_content`
  ——真执行，命中带「相对路径 + 行文本」进转录）
- [x] 既有 `subagent_does_not_inherit_declared_tools` 照旧绿（阶段声明扩不了权；期望集跟着改成三件）
- [x] 既有 `search.rs` 九条一字未改仍绿（值班长链的域规则 / 限幅 / 档位豁免不变）
