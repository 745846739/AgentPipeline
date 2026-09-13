# AgentPipeline 打包入口：两种形态（决策 155 / 156）。
#
#   make build         web 形态：前端 dist 内嵌进 agent-pipeline 单二进制
#   make run           build 后直接启动，浏览器开 http://127.0.0.1:8787（端口见配置 [server]）
#   make desktop       桌面形态：Tauri 2 壳打包出 AgentPipeline.app（壳内同源起服）
#   make desktop-run   桌面调试：debug 壳直接跑（窗口导航到内嵌服务）
#   make clean         清理构建产物（target / frontend/dist / node_modules / desktop target）
#
# 质量闸门不在这里：见 justfile（决策 147，just lint / just test）。
#
# **例外（决策 166）：** justfile 是闸门的权威定义，但 `just` 未必装在每台机器上
# （本仓库作者环境即未安装，决策 155 的 Makefile 正是因此存在）。故这里镜像一份
# 闸门入口，语义与 justfile 对齐，命名保持 `check-*` 以免与打包目标混淆：
#
#   make check          提交前必过全量（lint + test + frontend + e2e）
#   make check-lint     fmt --check + clippy -D warnings
#   make check-test     cargo test --workspace
#   make check-frontend 前端单元 + 类型检查 + 构建
#   make check-e2e      前端 E2E（含产物新鲜度守卫，决策 166）
#
# 两处定义必须同步；以 justfile 为准。

.PHONY: build frontend backend run desktop desktop-run clean check check-lint check-test check-frontend check-e2e

check: check-lint check-test check-frontend check-e2e

check-lint:
	cargo fmt --all -- --check
	cargo clippy --workspace --all-targets -- -D warnings

check-test:
	cargo test --workspace

check-frontend:
	cd frontend && npm test
	cd frontend && npm run check
	cd frontend && npm run build

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

run: build
	./target/release/agent-pipeline serve

# 桌面壳是独立 workspace（决策 156）：cargo 只在这里跑，tauri 依赖树不进根闸门。
# npx @tauri-apps/cli 会再次触发 release 编译（有缓存，秒级）并出 .app：
# crates/desktop/target/release/bundle/macos/AgentPipeline.app
desktop: frontend
	cd crates/desktop && npx --yes @tauri-apps/cli@^2 build

desktop-run: frontend
	cd crates/desktop && cargo build
	./crates/desktop/target/debug/agent-pipeline-desktop

clean:
	cargo clean
	rm -rf frontend/dist frontend/node_modules crates/desktop/target
