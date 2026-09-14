# AgentPipeline 打包入口：两种形态（决策 155 / 156）。
#
#   make build         web 形态：前端 dist 内嵌进 agent-pipeline 单二进制
#   make run           build 后直接启动，浏览器开 http://127.0.0.1:8788（端口见配置 [server]）
#   make desktop       桌面形态：Tauri 2 壳打包出 dmg（壳内同源起服；决策 168 起只出 dmg）
#   make desktop-run   桌面调试：debug 壳直接跑（窗口导航到内嵌服务）
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

.PHONY: build frontend backend run desktop desktop-run clean \
        check check-lint check-test check-frontend check-e2e \
        unit integration api e2e smoke fmt

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

# 只跑单元层（L1）
unit:
	cargo test --workspace --lib

# L2 集成（core tests/）
integration:
	cargo test -p agentpipeline-core --tests

# L3 API 契约（in-process axum router）
api:
	cargo test -p app --test api_contract

# L4 端到端场景
e2e:
	cargo test -p e2e

# 启动冒烟（spawn 真二进制，E2E-00）
smoke:
	cargo test -p app --test smoke

# 格式化（写回）
fmt:
	cargo fmt --all

clean:
	cargo clean
	rm -rf frontend/dist frontend/node_modules crates/desktop/target
