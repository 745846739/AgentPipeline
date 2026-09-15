# AgentPipeline 打包入口：两种形态（决策 155 / 156）。
#
#   make build         web 形态：前端 dist 内嵌进 agent-pipeline 单二进制
#   make run           build 后直接启动，浏览器开 http://127.0.0.1:8788（端口见配置 [server]）
#   make desktop       桌面形态：Tauri 2 壳打包出 dmg（壳内同源起服；决策 168 起只出 dmg）
#   make desktop-run   桌面调试：debug 壳直接跑（窗口导航到内嵌服务）
#   make icon          重新生成桌面应用图标（规格 theme-6-pixel.md §2.5）
#   make clean         清理构建产物（target / frontend/dist / node_modules / desktop target）
#
# 质量闸门也在这里（决策 147 / 166，**决策 168 起本文件是闸门的唯一权威定义**：
# justfile 已删除，`just` 未装在开发机上、维护两份定义只会漂移）：
#
#   make check          提交前必过全量（lint + test + frontend + e2e-frontend）
#   make check-lint     fmt --check + clippy -D warnings
#   make check-test     cargo test --workspace
#   make check-frontend 前端单元 + 类型检查 + 构建
#   make check-e2e      前端 E2E（含产物新鲜度守卫，决策 166）
#
# 分层子集（原 justfile 的目标，名字与语义原样保留）：
#
#   make unit           只跑单元层（L1）
#   make integration    core 的 L2 集成
#   make api            L3 API 契约（in-process axum router）
#   make e2e            L4 端到端场景
#   make smoke          启动冒烟（spawn 真二进制，E2E-00）
#   make fmt            格式化（写回，非 check）

.PHONY: build frontend backend run desktop desktop-run clean icon \
        check check-lint check-test check-frontend check-e2e \
        unit integration api e2e smoke fmt

# 工具链归一（决策 175）：本机 PATH 里 /opt/local/bin（MacPorts 自带 rust）排在
# ~/.cargo/bin（rustup）**之前**，裸 `cargo` 会落到另一套 rustc 上；两套 rustc 的
# 指纹不同，换一套就等于整棵依赖树重编（本机实测 10 分钟以上）。这里把 rustup 的
# shim 提到最前，配合仓库根的 rust-toolchain.toml 钉住 1.98.0。
# 用 `:=` 立即展开，取的是本 Makefile 解释时的 PATH，不受各 shell 差异影响。
export PATH := $(HOME)/.cargo/bin:$(PATH)

# 工具链**进程级**钉住（决策 175 收尾修正）：rustup 的 shim 按**当前目录**查找
# rust-toolchain.toml，而 cargo 编译第三方 crate 时的工作目录是
# ~/.cargo/registry/src/.../{crate}/ —— 那些 crate **自带** rust-toolchain.toml
# （实测 atoi 2.0.0 钉 1.57.0、sqlx 0.8.6 钉 1.78）。于是编译到 atoi 时 rustup
# 会**当场下载并切到 rustc 1.57.0**，而 1.57 不认识 cargo 传的 `--check-cfg`，
# 报 `Unrecognized option: 'check-cfg'` 直接失败；且该工具链会被永久装在机器上。
# RUSTUP_TOOLCHAIN 的优先级**高于**目录文件与环境无关，钉住它可一次性关掉这条路径。
export RUSTUP_TOOLCHAIN := 1.98.0

check: check-lint check-test check-frontend check-e2e

check-lint:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings

# 全量测试是**冷启动最贵的一步**（本机约 10 分钟）：它会为 22 个集成测试文件各
# 链接一个独立二进制，而每个二进制的链接输入含全部依赖 rlib（本机约 3.3 GB）。
# 日常改动用分层子目标（unit / integration / api / e2e / smoke）只编一层；
# 本目标留给提交前的那一次完整验证。
check-test:
	cargo test --workspace

check-frontend:
	cd frontend && npm test
	cd frontend && npm run check
	cd frontend && npm run build

# 前端 E2E：先跑产物新鲜度守卫（决策 166）——前端源码比 dist 新则重建 dist，
# 随后 cargo 增量重编二进制（frontend/dist 在 build.rs 的 rerun-if-changed 里，
# 故 dist 一变必重编）。没有这一步，改代码不重建就会得到「旧二进制的绿」。
check-e2e:
	bash scripts/e2e-artifacts.sh
	cd frontend && npx playwright test --project=chromium

build: frontend backend

# package-lock.json 比 node_modules 新时才重装依赖；锁文件变更后想强制重装可
# rm -rf frontend/node_modules。
frontend:
	cd frontend && if [ ! -d node_modules ] || [ package-lock.json -nt node_modules ]; then npm ci --no-audit --no-fund; fi
	cd frontend && npm run build

backend:
	cargo build --release

# 应用图标（规格 theme-6-pixel.md §2.5）：由 scripts/make-icon.mjs 的 32×32 坐标表生成
# crates/desktop/icons/{icon.png,icon.icns}。改图改脚本、重跑本目标，不直接改 PNG；
# 随后 --check 核取色表仍与规格 §2.1 逐字一致。
icon:
	node scripts/make-icon.mjs
	node scripts/make-icon.mjs --check

run: build
	./target/release/agent-pipeline serve

# 桌面壳是独立 workspace（决策 156）：cargo 只在这里跑，tauri 依赖树不进根闸门。
# npx @tauri-apps/cli 会再次触发 release 编译（有缓存，秒级）并按 tauri.conf.json
# 的 bundle.targets 出包：决策 168 起只出 dmg，路径
# crates/desktop/target/release/bundle/dmg/AgentPipeline_0.1.0_x64.dmg。
desktop: frontend
	cd crates/desktop && npx --yes @tauri-apps/cli@^2 build

desktop-run: frontend
	cd crates/desktop && cargo build
	./crates/desktop/target/debug/agent-pipeline-desktop

# 分层子集的三个可选参数（都可省略）：
#
#   PKG=<crate>    作用域收敛到单个 crate，只编/跑它（unit 层专用，其余层已自带 -p）
#   TESTS=<文件名> 只编/跑某一个测试文件（`--test <name>`）；默认 `--tests` 编全部。
#                  这一项省的是**编译与链接**，是大头（本机实测 core 的 L2 全部
#                  二进制 3m41s vs 单个 market 58s）；FILTER 省的是**执行**，很小
#   FILTER=<名称>  cargo 的用例名过滤，只跑名字匹配的用例
#
# 牙齿检查（停用某个防护 → 确认对应用例变红 → 恢复）用
# `make integration TESTS=market FILTER=<用例名>`，比 `make integration` 快一个量级。
#
# **作用域收敛一律走 PKG，不要写 `make unit -p <crate>`**：make 会把 `-p` 当成
# 自己的 `--print-data-base` 吞掉——① cargo 收不到作用域参数，实际跑的是整
# workspace（3 个测试二进制而非 1 个）；② 近 1900 行 make 数据库被 dump 到 stdout；
# ③ `<crate>` 被当成另一个 target，报 `No rule to make target` 并**以退出码 2
# 结束**。该调用若串在 `&&` 之后，后面的闸门步骤会被静默截断（实测 2026-09-15：
# 一个会话用它跑了 14 次，每次都误以为是「只跑 core」）。
test_filter = $(if $(FILTER),-- $(FILTER),)
test_scope  = $(if $(TESTS),--test $(TESTS),--tests)

# 只跑单元层（L1）
unit:
	cargo test $(if $(PKG),-p $(PKG),--workspace) --lib $(test_filter)

# L2 集成（core tests/）
integration:
	cargo test -p agentpipeline-core $(test_scope) $(test_filter)

# L3 API 契约（in-process axum router）
api:
	cargo test -p app --test api_contract $(test_filter)

# L4 端到端场景
e2e:
	cargo test -p e2e $(test_filter)

# 启动冒烟（spawn 真二进制，E2E-00）
smoke:
	cargo test -p app --test smoke $(test_filter)

# 格式化（写回）
fmt:
	cargo fmt --all

clean:
	cargo clean
	rm -rf frontend/dist frontend/node_modules crates/desktop/target
