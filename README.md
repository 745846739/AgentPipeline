# AgentPipeline

Kanban 式流水线驱动的本地多 agent 开发管线：`init → architect-design → develop-design / test-design（并行）→ sync-check → develop → review → test → merge → done`。

设计文档（唯一事实来源）在 [docs/](docs/README.md)；本文只讲怎么把代码跑起来。

## 环境

- Rust 1.80+（当前验证于 1.98）
- `git` 能力由 **git2（libgit2 绑定）** 提供，无需系统 git（决策 12；测试 fixture 仍走系统 git CLI 作脚手架，决策 146 修订）
- SQLite 由 sqlx 内嵌编译，无需单独安装

## 构建与运行

```bash
make build    # 前端依赖（按需）→ vite build → 内嵌 dist → cargo build --release
make run      # 等价于 build + 启动
```

产物是单二进制 `./target/release/agent-pipeline`：前端 dist 在编译期内嵌（决策 155），
axum 同源托管 UI 与 API，浏览器打开 `http://127.0.0.1:8787` 即用（端口见配置 `[server]`，
`--port` 可覆盖）。手动等价：`cd frontend && npm ci && npm run build && cargo build --release`。

- 不装 Node 也能 `cargo build --release`：API 照常可用，访问 `/` 会得到「前端未构建」提示页（决策 155）。
- 前端热更开发：`cd frontend && npm run dev`（vite 把 API 代理到本机 axum，见 frontend/vite.config.ts）。

数据全部落在 `~/.agentpipeline/`（可用环境变量 `AGENTPIPELINE_HOME` 覆盖，测试即靠它隔离）。
首次启动后到 `POST /providers` 配置一个 provider，才能创建任务（未配置时创建任务会明确报错，决策 56）。

## 质量闸门

```bash
just lint          # cargo fmt --check + cargo clippy -D warnings（提交前必过）
just test          # 全量测试（L1 + L2 + L3 + 冒烟 + L4）
just unit          # 只跑单元层
just integration   # core 的 L2 集成（游标 / git / scheduler）
just api           # L3 API 契约（in-process axum router）
just e2e           # L4 场景
just smoke         # 启动冒烟（spawn 真二进制）
```

当前状态：**481 个用例全过**（另有 1 个 `#[ignore]` 真 LLM 冒烟），`fmt` / `clippy -D warnings` 干净。前端另有 85 个 vitest + 2 条 playwright E2E，在 `frontend/` 下单独跑（见 [docs/testing.md](docs/testing.md) §9）。

## 代码结构

```
crates/core/     核心库
  pipeline/      图拓扑、条件边路由、落点表、游标谓词
  agent/         LLM 接缝、工具执行、结构化输出解析、FileToolPolicy、上下文压缩、prompt 组装
  storage/       SQLite（sqlx migrations）、游标生命周期、冲突比对、旁路动作事务
  scheduler/     KanbanScheduler.tick 六项职责
  git.rs         git2 封装（worktree / rebase / 合入写回 / 重置清理 / 项目探测，决策 12）
  actions.rs     allowed_actions 权威表与端点配对
  sse.rs         唯一事件流的类型契约
crates/app/      二进制 + axum router（lib 形态供 tower oneshot 测试）
crates/testkit/  测试基建：临时 home、git fixture、FakeAgent、断言助手（决策 146 / 148）
tests/e2e/       L4 场景
```

四条可测试性接缝（决策 143）已就位：`Clock` trait、`AGENTPIPELINE_HOME`、`ProcessKiller` trait、手动驱动的 `scheduler.tick()`。

**实现进度与剩余工作见 [docs/testing.md](docs/testing.md) §11。**
