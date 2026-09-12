# AgentPipeline 本地质量闸门（决策 147）
#
# lint  = fmt --check + clippy -D warnings（提交前必过）
# test  = 全量测试（L1 单元 + L2 集成 + L3 API + 冒烟）
# e2e   = L4 场景矩阵（testing.md §8）
# smoke = 仅 spawn 真二进制的启动冒烟（E2E-00）
# frontend-e2e = 前端 playwright 双冒烟（真后端 + mock LLM + 临时 home；只 Chromium）

default: lint test

# 格式 + 静态检查（决策 139 dogfood：lint 是确定性闸门）
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings

# 全量测试
test:
    cargo test --workspace

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

# 前端 E2E（真 axum 后端 + mock LLM + 临时 home；只 Chromium，决策 144 / 151）
frontend-e2e:
    cd frontend && npx playwright test --project=chromium

# 格式化
fmt:
    cargo fmt --all
