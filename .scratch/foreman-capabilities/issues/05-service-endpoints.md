# 05: D 层本服务接口接进确认钮（按领域分批）

**What to build:** 把本系统的接口变成工具面，**一族一个工具 + 动作参数**（决策 207），粒度对着
`allowed_actions` 的类型走——与既有动作集同构。分批推进，每批一张提交：

1. **任务**：建任务 / resume / retry / cancel / 拍板（judge 分歧、review）/ 合入（merge decision）；
2. **配置**：改阶段配置（provider、温度、超时、技能声明）；
3. **技能与市场**：装技能 / 删技能。

读端点直接执行；写端点**恒为提议**，与档位无关（决策 206）——所以本票的工具**不进**
`FOREMAN_ENV_TOOLS`，它们在 `FOREMAN_SERVICE_WRITE_TOOLS` 里。

**排除三项**（决策 207，**不实现**）：重置配对令牌、局域网开关、仓名单（`kanban_market_repos`）增删。
判据是「改的是**谁能访问这台机器**」——让模型能提议它们等于让它能给自己开门，而「内置 ≠ 放行」
（`routes/market.rs:56` 的原话）本来守的就是这条线。

**Blocked by:** 03

**Status:** done

- [x] 三个领域各自一族工具，参数与既有端点的入参**同构**（不发明第二套参数语言）
- [x] 写端点一律提议；读端点直通（有测试按族断言）
- [x] 排除三项**没有**对应的工具（测试断言清单里不含它们，且执行点也拒）
- [x] 执行路径走既有端点（同一套校验；提议里的参数过不了校验时执行失败）
- [x] `FOREMAN_SERVICE_WRITE_TOOLS` 清单与实现同源
- [x] 工具描述里的参数说明足够模型选对动作（粒度代价的缓解：错选由确认钮兜住，但别让它频繁错选）

## 交付

**一族一个工具 + 动作参数**（决策 207④），三个名字进 `SERVICE_WRITE_TOOLS`：

| 工具 | 动作 | 参数（与既有端点入参同形） |
|---|---|---|
| `task` | `create` / `resume` / `retry` / `cancel` / `review` / `merge` | `project_id` / `title` / `depends_on` / `review_mode`；`task_id` + `resume_action` / `cursor_id` / `target_stage` / `target_node` / `input`；`approved` / `comments`；`decision` |
| `config` | `set` / `delete` | `stage` + `provider_id` / `temperature` / `max_tokens` / `persona_path` / `persona_append` / `env_mode` / `tools_json` / `skills_json` / `node_overrides_json` / 两个超时 |
| `skills` | `install` / `delete` | `install`：`path`（本地技能目录）+ `overwrite`；`delete`：`name` |

- **写端点恒为提议、不读档位**（`needs_confirmation` 对任何档位都返回真，有 L1 钉住），
  且有一轮实测：`tests/foreman.rs::the_service_write_tools_propose_even_under_the_auto_tier`
  ——把值班长配成 `auto`，三个工具各提一条、任务与阶段配置**一个字节都没动**。
- **执行 = 直接调那个端点的处理器函数**（不是发一次 in-process HTTP——那会重新过一遍跨源与配对层，
  而这两层判的是「谁在门外」，这次调用已经在门内）。端点的错误**逐字带回**。
- **排除三项没有工具名**：清单里只有 17 个名字，`pairing` / `lan` / 仓库名单相关的名字一个都没有；
  库里有条工具名对不上的提议时，执行报「这条提议的工具还没有接线」且**没有任何副作用**
  （`the_door_opening_actions_have_no_tool_at_all`）。
- **参数过不了校验 → 执行失败且提议不消耗**（`a_proposal_that_fails_the_endpoint_validation_stays_pending`，
  报文就是端点自己那一句「项目不存在」）。
- `skills` 的 `install` 读端点回的 `failed` **字段**（不是匹配报文里的字样）：批量端点一项失败时
  HTTP 仍是 200，只看状态码会把「没装上」报成「已执行」。
