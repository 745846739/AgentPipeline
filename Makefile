# AgentPipeline 打包入口：前端 dist 经 build.rs 内嵌进 agent-pipeline 单二进制（决策 155）。
#
#   make build    装前端依赖（按需）→ vite build → cargo build --release
#   make run      build 后直接启动，浏览器开 http://127.0.0.1:8787（端口见配置 [server]）
#   make clean    清理构建产物（target / frontend/dist / node_modules）
#
# 质量闸门不在这里：见 justfile（决策 147，just lint / just test）。

.PHONY: build frontend backend run clean

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

clean:
	cargo clean
	rm -rf frontend/dist frontend/node_modules
