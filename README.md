# AgentPipeline

Kanban 式流水线驱动的本地多 agent 开发管线：`init → architect-design → develop-design / test-design（并行）→ sync-check → develop → review → test → merge → done`。

设计文档（唯一事实来源）在 [docs/](docs/README.md)；本文只讲怎么把代码跑起来。

## 环境

- Rust 1.80+（当前验证于 1.98）。仓库根的 `rust-toolchain.toml` **钉住精确版本 `1.98.0`**
  （决策 175）——不用 `stable` 通道：`stable` 会让 rustup 在构建途中把工具链换掉，造成
  整棵依赖树重编且闸门中断。首次构建会自动下载该版本（一次），此后固定不变。
- `git` 能力由 **git2（libgit2 绑定）** 提供，无需系统 git（决策 12；测试 fixture 仍走系统 git CLI 作脚手架，决策 146 修订）
- SQLite 由 sqlx 内嵌编译，无需单独安装

## 构建与运行

```bash
make build    # 前端依赖（按需）→ vite build → 内嵌 dist → cargo build --release
make run      # 等价于 build + 启动
```

产物是单二进制 `./target/release/agent-pipeline`：前端 dist 在编译期内嵌（决策 155），
axum 同源托管 UI 与 API，浏览器打开 `http://127.0.0.1:8788` 即用（端口见配置 `[server]`，
`--port` 可覆盖）。手动等价：`cd frontend && npm ci && npm run build && cargo build --release`。

- 不装 Node 也能 `cargo build --release`：API 照常可用，访问 `/` 会得到「前端未构建」提示页（决策 155）。
- 前端热更开发：`cd frontend && npm run dev`（vite 把 API 代理到本机 axum，见 frontend/vite.config.ts）。

### 桌面形态（Tauri 壳，决策 156）

```bash
make desktop       # 出 dmg（crates/desktop/target/release/bundle/dmg/）
make desktop-run   # debug 壳直接跑，窗口导航到内嵌同源服务
```

桌面壳只是外壳：壳内调用 `app::serve(ServeOptions)` 起服，**端口不指定**——用 `[server] port`
（缺省 8788），于是二维码地址、手机书签、桌面窗口地址跨重启都指着同一个号码（决策 213；此前是
`port_override = Some(0)`，每次重启换一个临时端口，手机存过的网址就失效了）。真被别的程序占着时
退让到临时端口而不是拒绝开窗（`port_fallback_to_ephemeral`），退让会经 `/server-info` 的
`port_source` 报到「手机访问」页上。窗口加载 `http://127.0.0.1:{port}`（同源零 CORS），传输层与
web 形态完全一致（决策 153）。
壳是独立 workspace（自带 Cargo.lock），tauri 依赖树不进 `make check-lint` / `make check-test` 闸门。

打包边界（决策 168）：`bundle.targets` 只列 `dmg`——dmg 里已含 `.app`（拖入「应用程序」即得，
本机开发也不必再从 `bundle/macos/` 取），故不再额外落一份裸 `.app` 到产物目录。

数据全部落在 `~/.agentpipeline/`（可用环境变量 `AGENTPIPELINE_HOME` 覆盖，测试即靠它隔离）。
首次启动后到 `POST /providers` 配置一个 provider，才能创建任务（未配置时创建任务会明确报错，决策 56）。

### 手机扫码访问（决策 167）

桌面壳或 web 形态都能让手机通过局域网接入：打开应用内「**手机访问**」页（`#/share`）扫二维码即可。
只读页面（看板 / 会话 / 指标 / 分享页）扫开就能看；**写操作与对讲台需配对**（见下方警告）。

```bash
# web：绑定全网卡后，到「手机访问」页扫码
./target/release/agent-pipeline serve --host 0.0.0.0

# 桌面壳：设环境变量开启局域网（默认仍只绑回环）
AGENTPIPELINE_LAN=1 make desktop-run
```

手机加载的页面与 API **同源**（内嵌 dist 同源托管，决策 155），因此**无需** `--allowed-origin`：
跨源防护（决策 128）只拦异源写请求，同源写请求恒带客户端头，天然通过。
地址由后端枚举网卡择优给出（私网优先、VPN/Docker 虚拟网卡降级，决策 167）；
若只绑了回环地址，页面会提示「127.0.0.1 在手机上指向手机自己」并给出开启步骤。

> **写请求与对话需配对**（决策 167 推迟、决策 182⑦ 落地）：绑定**非回环**地址时启用**配对令牌**——
> 只读页面（看板 / 会话 / 指标 / 分享页）在局域网里照旧直接可看，**所有写请求与全部
> `/foreman/*`（对讲台）需要 `X-AgentPipeline-Token`**，缺失或不匹配即 403「这台设备还没配对」。
> 令牌由服务端生成、**长期有效**（不随启动重生成）：本机 `GET /pairing/token` 读取（**仅回环可读**），
> `POST /pairing/reset` 一键重置换一枚；配对 URL 的形状是 `{base}/?pair={token}`。
> **默认回环绑定（本机使用）不要求任何配对**。两点仍需注意：只读页面里的会话与任务内容是明文展示的、
> 同网段可看；且服务端这层之外没有别的防线，请勿在公共 Wi-Fi 下开启，更稳妥可用 SSH 隧道 / Tailscale。
> 前端消费 `?pair=` 这条链路（`main.ts` 启动时收取 → localStorage → 从地址栏抹掉 → 之后作为
> `X-AgentPipeline-Token` 头带上，含只读 GET），以及令牌与局域网态势的完整交代，
> 记在 [docs/operations.md](docs/operations.md) §12.16。

### 局域网访问（决策 157）

若要让**另一台电脑的浏览器**直接打开本机页面（页面 origin 与 API 不同源），才需要显式放行该 origin：

```bash
./target/release/agent-pipeline serve --host 0.0.0.0 --allowed-origin http://192.168.1.10:8788
```

`--allowed-origin` 可重复（配置文件等价写法 `[server] allowed_origins = [...]`，非法值启动即报错）；
`127.0.0.1` / `localhost` 恒放行，前缀伪装（`...:8788.evil.com`）始终被拦。
注意：服务能触发真实 LLM 调用并读取全部会话，暴露到局域网前请自行评估网段安全（更稳妥可用 SSH 隧道 / Tailscale）。

## 质量闸门

**提交前必过**（决策 147 / 166）：`make check` = `lint` + `test` + `frontend` + `e2e`。

```bash
make check           # 提交前必过：以下四项全跑
make check-lint      # cargo fmt --check + cargo clippy -D warnings
make check-test      # 全量测试（L1 单元 + L2 集成 + L3 API + L4 场景 + 冒烟）
make check-frontend  # 前端：vitest + svelte-check + vite build
make check-e2e       # 前端 E2E（playwright 17 例；前置产物新鲜度守卫）
# 分层子集
make unit            # 只跑单元层
make integration     # core 的 L2 集成（游标 / git / scheduler）
make api             # L3 API 契约（in-process axum router）
make e2e             # L4 场景
make smoke           # 启动冒烟（spawn 真二进制）
make fmt             # 格式化（写回）
# 构建缓存清理（非闸门，随时可跑）
make sweep           # 只清可再生的缓存（增量缓存 / deps/*.rcgu.o / cargo doc），保留第三方 rlib
```

**Makefile 是闸门的唯一权威定义**（决策 168）：原先并存的 `justfile` 已删除——
开发机上未装 `just`，两份定义只会在改动时漂移，而 Makefile 是实际被执行的入口。
`justfile` 独有的分层目标（`unit` / `integration` / `api` / `e2e` / `smoke` / `fmt`）
已按原语义搬进 Makefile，名字不变。

**构建缓存会只增不减**，而这件事 `cargo-sweep` 看不见——它按**访问时间**判「过时」，
可这里的堆积按时间算全是**新**的（实测 `--stamp` 报 0、`--time 4` 只有 61 MiB，而项目
实占 29 GB）。`make sweep` 只删**可再生**的中间产物：增量缓存、`deps/*.rcgu.o` 这类
增量编译每次会话产生、cargo 从不回收的一次性目标文件、`cargo doc` 产物；第三方
rlib / rmeta 一律保留，故清完不必重编整棵依赖树。`DRY=1 make sweep` 只报将删什么。
核选项仍是 `make clean`（连第三方产物一起删，下次是冷编，本机实测约 20 分钟）。

**产物新鲜度守卫（决策 166）：** `check-e2e` 跑用例前先确保被测对象是当前源码——
前端源码比 `frontend/dist` 新则重建 dist，随后 `cargo build -p app`（dist 变化经
`crates/app/build.rs` 的 `rerun-if-changed` 传递，二进制必然跟着重编）。没有这一步，
改了代码不重建就会得到「旧二进制的绿」。

**本项目无 CI**（无 `.github/workflows/`）：闸门靠本地执行，这是当前形态而非遗漏。

当前状态：**Rust 848 个用例全过**（另有 2 个 `#[ignore]` 真 LLM 冒烟：单节点 + 全流程），
`fmt` / `clippy -D warnings` 干净；前端 **290 个 vitest 全过** + `svelte-check` 0 error /
0 warning + **33 条 playwright E2E 全过**（共 35 例，2 例截图证据默认 skip，见下）（`make check-e2e`）。

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
