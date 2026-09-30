# AgentPipeline 打包入口：两种形态（决策 155 / 156）。
#
#   make build         web 形态：前端 dist 内嵌进 agent-pipeline 单二进制
#   make run           build 后直接启动，浏览器开 http://127.0.0.1:8788（端口见配置 [server]）
#   make desktop       桌面形态：Tauri 2 壳打包出 dmg（壳内同源起服；决策 168 起只出 dmg）
#   make desktop-run   桌面调试：debug 壳直接跑（窗口导航到内嵌服务）
#   make icon          重新生成桌面应用图标（规格 theme-6-pixel.md §2.5）
#   make clean         清理构建产物（target / frontend/dist / node_modules / desktop target）
#   make sweep         只清**可再生**的缓存（增量缓存 / deps/*.rcgu.o / cargo doc），
#                      保留第三方 rlib 与 rmeta；`DRY=1 make sweep` 只报将删什么
#
# 质量闸门也在这里（决策 147 / 166，**决策 168 起本文件是闸门的唯一权威定义**：
# justfile 已删除，`just` 未装在开发机上、维护两份定义只会漂移）：
#
#   make check          提交前必过全量（lint + test + frontend + e2e-frontend）
#   make check-lint     fmt --check + clippy -D warnings
#   make check-test     cargo test --workspace
#   make check-frontend 前端单元 + 类型检查 + 构建
#   make check-e2e      前端 E2E（含产物新鲜度守卫，决策 166）
#   make hooks          安装 pre-commit 闸门（决策 348，core.hooksPath → scripts/hooks）
#
# ── 谁在哪儿跑（决策 331）：别再按「一律本机跑全量」的老口径办 ──────────────
# 闸门跑两处（决策 330），分工是定好的：
#
#   本地必跑：`make check-lint`（暖树上实测 18 秒）+ **改动所在那一层的窄跑**
#             （`make unit PKG=<crate>` / `make integration TESTS=<模块>`）。这一档执行是
#             秒级——决策 178 的增量加 218 的合并二进制让它便宜到没理由省；省掉它换来的
#             只是「推上去等 CI 说 clippy 挂了」的往返。
#   交给 CI ：完整的 `make check`。**尤其 check-e2e**——33 条 spec 在 `workers: 1` 下串行，
#             是本机最大的时间黑洞。CI 是 `x86_64-linux`，**与 106 上的生产运行时同族**，
#             故它对「生产会不会坏」的说服力比本机那套 darwin 形态更强。
#   只有本机：`make desktop`（Tauri dmg 是 macOS 专属）与 `#[cfg(target_os = "macos")]`
#             那类分支——CI 永远看不到它们（反过来，linux 那一侧是本机的盲区）。
#   推完必读结论：`gh run list --workflow check`；红了看 `gh run view --log-failed`。
#             **部署有没有发生，看 `deploy-106` 有没有起 run**——check 红就是没部署
#             （部署挂在 check 的结论上，决策 330），不是「再等等」。
#   一个例外：**首次 CI 跑绿之前**仍在本机跑完整 `make check`——linux 侧还没被证明过
#             （`cfg(not(target_os = "macos"))` 那些分支从没被 clippy 看过）。
#
# 分层子集（原 justfile 的目标，名字与语义原样保留）：
#
#   make unit           只跑单元层（L1）
#   make integration    core 的 L2 集成
#   make api            L3 API 契约（in-process axum router）
#   make e2e            L4 端到端场景
#   make smoke          启动冒烟（spawn 真二进制，E2E-00）
#   make fmt            格式化（写回，非 check）

.PHONY: build frontend backend run desktop desktop-run clean sweep icon \
        check check-lint check-test check-frontend check-e2e \
        unit integration api e2e smoke fmt hooks

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

# 全量测试是**冷启动最贵的一步**：它为每个集成测试文件各链接一个独立二进制。
# 决策 218 把 33 个集成文件按 crate 合成 3 个二进制（测试二进制 37 → 7），实测
# core 的 L2 编译 CPU 从 445 CPU-s 降到 317 CPU-s（−29%）；合并的收益来自
# 「每个 crate root 都要付一次的固定成本」——实测往合并二进制里多塞一个文件只要
# 约 0.15 CPU-s，而单独建一个文件要 1.7–2.6 CPU-s。
#
# 两处**旧注释已被实测推翻**，别再照抄：① 冷构建不是「约 10 分钟」——本机实测
# 约 20 分钟（2026-09-18）；② 「链接是大头、链接输入约 3.3 GB」不成立——链接只占
# 单个测试二进制的 2–3%（0.43–0.72s，且不随二进制体积增长），`deps/*.rlib`
# 合计 1.03 GB（642 个），链接器只抽取用到的成员。改链接器曾是候选方向，已否决。
#
# 日常改动用分层子目标（unit / integration / api / e2e / smoke）；
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

# 分层子集的三个可选参数（都可省略）。**决策 218 起集成测试按 crate 合成单一二进制**，
# 所以「按文件收敛**编译**」不再可能——每个测试文件现在是一个**模块**：
#
#   PKG=<crate>    作用域收敛到单个 crate，只编/跑它（unit 层专用，其余层已自带 -p）
#   TESTS=<模块名> 收窄到某一个测试文件的用例，走**用例名前缀**过滤（`-- <模块>::`）。
#                  即：**编译不再收敛，执行仍然收敛**——改一个文件要重编整个二进制，
#                  但编好之后只跑一个文件几乎不花时间。这一收支见 check-test 处的实测。
#   FILTER=<用例名> 再收窄到单个用例。与 TESTS 同用时二者串成 `<模块>::<用例>` 作为
#                  **一个子串**交给 harness，所以此时 FILTER 要写**完整用例名**——写半截
#                  （如 `rebase`）会因中间隔着模块前缀而匹配不到（实测 0 条，不是报错，
#                  静默通过是这里唯一的坑）。只给 FILTER 时仍是全二进制的子串匹配，行为同旧。
#
# 牙齿检查（停用某个防护 → 确认对应用例变红 → 恢复）用
# `make integration TESTS=market FILTER=<用例名>`。
#
# **作用域收敛一律走 PKG，不要写 `make unit -p <crate>`**：make 会把 `-p` 当成
# 自己的 `--print-data-base` 吞掉——① cargo 收不到作用域参数，实际跑的是整
# workspace（3 个测试二进制而非 1 个）；② 近 1900 行 make 数据库被 dump 到 stdout；
# ③ `<crate>` 被当成另一个 target，报 `No rule to make target` 并**以退出码 2
# 结束**。该调用若串在 `&&` 之后，后面的闸门步骤会被静默截断（实测 2026-09-15：
# 一个会话用它跑了 14 次，每次都误以为是「只跑 core」）。
# 用例级过滤（unit 层用；名字形如 `模块::用例`）
case_filter  = $(if $(FILTER),-- $(FILTER),)
# 文件模块 + 用例。决策 218：集成测试按 crate 合并成单一二进制后，每个测试文件是
# 一个**模块**，用例全名形如 `<文件模块>::<用例>`，故「按文件收敛」靠用例名前缀，
# 而不是 `--test <文件>`（那个不再存在）。
suite_filter = $(if $(TESTS),$(TESTS)::,)$(FILTER)
test_filter  = $(if $(strip $(suite_filter)),-- $(suite_filter),)

# 只跑单元层（L1）
unit:
	cargo test $(if $(PKG),-p $(PKG),--workspace) --lib $(case_filter)

# L2 集成（core tests/integration/，单一二进制）
integration:
	cargo test -p agentpipeline-core --test integration $(test_filter)

# L3 API 契约（in-process axum router）；合并后靠用例名前缀收敛到 api_contract 模块
api:
	cargo test -p app --test integration -- api_contract::$(FILTER)

# L4 端到端场景（e2e tests/integration/，单一二进制）
e2e:
	cargo test -p e2e --test integration $(test_filter)

# 启动冒烟（spawn 真二进制，E2E-00）；同上，靠前缀收敛到 smoke 模块
smoke:
	cargo test -p app --test integration -- smoke::$(FILTER)

# 格式化（写回）
fmt:
	cargo fmt --all

# 安装 pre-commit 闸门（决策 348）：core.hooksPath 指向仓库内的 scripts/hooks。
# 克隆 / 换机后跑一次 `make hooks` 即接管；脚本内容见 scripts/hooks/pre-commit
# （只跑 lint 层：Rust → check-lint，前端 → svelte-check；理由与范围写在那儿）。
hooks:
	git config core.hooksPath scripts/hooks
	@echo "pre-commit 闸门已接管（scripts/hooks/pre-commit，决策 348）"

clean:
	cargo clean
	rm -rf frontend/dist frontend/node_modules crates/desktop/target

# 缓存清理（`scripts/sweep-artifacts.sh`）：只删**可再生**的中间产物——增量缓存、
# `deps/*.rcgu.o` 这类只增不减的一次性目标文件、`cargo doc` 产物。第三方 rlib / rmeta
# 一律保留：它们是「改一处不必重编整棵依赖树」的前提（决策 178 的收益靠它兑现）。
#
# 为什么不并进 `clean`：`cargo clean` 连第三方产物一起删，下次是冷编——本机实测约
# 20 分钟（见 check-test 处的更正）。本目标把「清缓存」与「清产物」分开，`clean` 仍是
# 核选项、本目标是日常可重跑的那一档。
#
# 为什么不用 `cargo-sweep`（本机已装、仓库里也留着它的 sweep.timestamp）：它按**访问
# 时间**判「过时」，而这里的堆积按时间算全是**新**的——2026-09-18 实测它只报 61 MiB
# （`--stamp` 更是 0），而当时项目实占 29 GB、其中 18 GB 正是本脚本删掉的那类。
#
# 实测支撑（2026-09-18，决策 218 之后）：删净 root 的 9.2 GB 增量缓存与 131,924 个
# `.rcgu.o` 后，`cargo build --workspace` 与 `cargo test --workspace --no-run` 都仍是
# **0 个 crate 编译的空转**，`cargo test -p agentpipeline-core --lib` 469 passed。
# 桌面壳是独立 workspace（决策 156），两个 target 树都在扫描范围内。
# 完整理由、坑（131k 文件必须走 find 而非通配符）与逆向验证见该脚本文件头。
# `DRY=1 make sweep` 只报将删什么，不动盘。
sweep:
	bash scripts/sweep-artifacts.sh
