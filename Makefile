# AgentPipeline 打包入口：两种形态（决策 155 / 156）。
#
#   make build         web 形态：前端 dist 内嵌进 agent-pipeline 单二进制
#   make run           build 后直接启动，浏览器开 http://127.0.0.1:8787（端口见配置 [server]）
#   make desktop       桌面形态：Tauri 2 壳打包出 AgentPipeline.app（壳内同源起服）
#   make desktop-run   桌面调试：debug 壳直接跑（窗口导航到内嵌服务）
#   make clean         清理构建产物（target / frontend/dist / node_modules / desktop target）
#
# 质量闸门不在这里：见 justfile（决策 147，just lint / just test）。

.PHONY: build frontend backend run desktop desktop-run clean

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
