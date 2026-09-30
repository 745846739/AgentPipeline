# 01: foreman.rs 拆成 pipeline/foreman/ 六文件 + TurnRegistry

**What to build:** 决策 351——`crates/core/src/pipeline/foreman.rs`（4148 行）纯文件搬家成
`pipeline/foreman/` 目录：registry / catalog / briefing / conversation / runner / attribution
六个文件，mod.rs re-export 保持外部 import 路径不变。附注：注册表 + 守卫捆成具名
`TurnRegistry`，计数纪律获得 crate 内单测面。

**Blocked by:** None

**Status:** done（已实现，决策 351）

- [x] 六文件切分按 spec.md 的归属表；切分线以现有结构为准，不重排任何函数体
- [x] `mod.rs` re-export 后 `routes/foreman.rs` 与仓内其余 import 行**零改动**
      （workspace `cargo check` 绿即证：app / 集成测试 / sse / config / types 的全部
      `pipeline::foreman::*` 引用一字未动；git 把 `foreman.rs → foreman/runner.rs`
      识别为重命名）
- [x] 内联测试跟随各自模块——**口径收口**：原 foreman.rs 没有 `#[cfg(test)]` 内联测试
      （盘点时的 ~700 行「内联测试」实为 attribution 与 helpers 等真实代码），
      foreman 用例全部住在 `tests/integration/foreman*.rs`，未动
- [x] `TurnRegistry` 具名类型：`begin` 发凭据 / Drop 摘 / `in_flight` 读数三件进类型接口；
      `FOREMAN_TURNS` 从裸 `LazyLock<Mutex<HashMap>>` 换成 `LazyLock<TurnRegistry>`；
      语义逐字不变（计数、减到 1 以下即删格）。三条 crate 内单测：
      同班两轮并跑先退不摘另一轮（bool 按格覆盖错法）、会话分格独立、
      生产接线（begin_foreman_turn / foreman_turn_in_flight）走同一份类型接口
- [x] `FOREMAN_TOOL_SPECS` 本体、冻结断言、`Script::for_foreman()` 槽位一字不动
- [x] 验证：`cargo fmt --all -- --check` + `cargo clippy --workspace --all-targets -D warnings`
      绿；`cargo test --workspace` 全绿（core 库 656 = 653 + 新 3，core 集成 463，
      app 集成 224）；e2e foreman 族（talk / talk-stop / talk-watch / stewardship）
      46 条全过

**注记（留给后来者）**：

- runner.rs 落在 ~2500 行而不是盘点时报的 ~1300——那份估算把运行器常量 / 人格 /
  ForemanTrace·Segment·Turn 形状 / WatchFailureState 都算到别的文件去了；按「编排+落库 +
  它自己的常量与形状」的归属表它们都归 runner。六文件形状与接口面才是本票的验收物，
  行数是估算不是契约。
- 拆分中抬升的可见性全部是 `pub(super)`（模块内兄弟互用），无一升到 `pub`——外部
  接口面与拆分前逐项相同。
