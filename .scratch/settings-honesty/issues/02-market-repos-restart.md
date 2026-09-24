# 02: 界面保存的仓名单跨重启读回（决策 257 落档）

**What to build:** `Store::market_repos_override()`（`crates/core/src/storage/market_repos.rs:19`）
**全仓零调用方**——只有 `set_` / `clear_` 被路由调用（`routes/market.rs:137,148`）。启动时
`serve.rs:471` 读的是 `config.market.resolved_repos()`。故界面保存的仓名单**落进 DB 后没人读回**，
**重启静默回落到 `config.toml`**。

**这不是取舍，是实现漏掉的一步**：存储层的 doc（`market_repos.rs:12-19`）明写「`Some(vec![])` =
用户显式清空 = 不从任何仓安装；`None` = 没保存过，读配置」，并注明「这条区分是迁移 0009 的注释里
**已经写明的**，不在新表上丢掉」；术语表 `glossary.md`「技能市场」条同口径（「**显式清空 ≠ 未保存过**」）。
而重启之后这两者**变得无法区分**——正是那句话说要避免的。**对照**：`server_bind_override`
**是被读的**（`serve.rs:429` 经 `resolve_bind_host`），两者同构（同迁移批、同为单行表
`CHECK (id = 1)`）。

**Blocked by:** None（可立即开始）

**Status:** done（已实现）

- [x] `serve.rs` 启动路径：读 `store.market_repos_override().await?`，`Some(v)` 时**同时**
      `set_market_override(v)`，`configured_repos` 保持 `config.market.resolved_repos()` 那一份
- [x] **形状要点（别只当 `configured_repos`）**：若只把 DB 那份作为 `configured_repos` 传进去，
      `repos_json`（`market.rs:105-113`）的 `origin` 判据是 `state.market_override().is_some()`，会
      **显示 `"config"`**——而那个字段的 doc 明写它的用途是「用户改 `config.toml` 却发现『改了没用』
      时，答案必须在这一页上看得见」。只当 configured 会**制造**一个正好相反的困惑
      （用户没改配置，界面却说是配置定的）
- [x] 补一条**重启见证**用例（现有 `repos_config_is_a_two_level_override` 是同一进程内的：保存 → 读 →
      清，**见证不了重启**）：保存一份 → 用同一个 `store` 重建 `AppState`（照 `serve` 的启动形状）→
      断言名单仍在**且** `origin === "settings"`；再断言 `DELETE` 之后重建回落到 `config.toml` 那一份
- [x] `docs/testing.md` L3 用例目录加 row（锚决策 257）
- [x] `docs/glossary.md`「技能市场」条的「保存即生效」口径**不必改**（它说的是进程内免重启，本来就对）；
      若读着容易误读成「跨重启也生效」，就地补一句「重启后由启动路径读回（决策 257）」

**唯一的行为变化**：保存过的仓名单跨重启存活；`GET /market/repos` 的 `origin` 在那种情形下从
`"config"` 变 `"settings"`（**这是修正，不是回归**）。

**明确不做**：不改 `DELETE` 的回落语义；不改 `market_repos_override` 的空表 / `None` 区分；不动
`listings`（进程内、不过期，doc 已写明「重启之后重新 `head()` 是对的」）；不给 `[market] github_repos`
做界面（那是决策 56 的边界）。

---

## 实施收尾（2026-09-23）

`make check` 全绿（`check-test` 178 passed，含本票新增的那条）。

**落点**：`serve.rs` 启动路径加 `store.market_repos_override().await?`，经新增的
`AppState::with_market_override(Option<Vec<String>>)` 装成界面那一级；`configured_repos`
保持 `config.toml` 那一份。这就是票面「形状要点」要求的甲-2 而不是甲-1。

**见证用例的牙齿已验**：把 `with_market_override` 那一行拿掉重跑，
`settings_saved_market_repos_survive_a_real_restart` 当场红，失败报文正是本票要抓的症状——

```
界面保存的名单必须跨重启存活：{"origin":"config",...,"repos":["config/level"]}
```

——保存过的 `Obra/Superpowers` 消失、回落到 `config.toml` 那一级、`origin` 谎报 `config`。
三件事一次全中。（Rust 侧另生一条 `unused` 警告，说明那个绑定当时确实没人用。）

**顺带订正票面的一处低估**：本票原写「补一条重启用例」，但进程内的用例**看不见**这件事
——`PUT /market/repos` 写 DB 与 `serve` 启动读 DB 在同一个进程里是串起来的，`AppState` 当场
就装上了 override。故见证了**真进程重启**（`restart_recovery.rs` 那套 `kill` + 同 home 再起
的脚手架），这也与它的同构参照物 `server_bind_override` 一致（那个也是界面写 DB、启动读回）。
