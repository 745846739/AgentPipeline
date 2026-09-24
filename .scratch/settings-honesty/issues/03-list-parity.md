# 03: 三份清单的同一性用一条定值探针钉住（决策 258 落档）

**What to build:** 一个全局设置字段的存在被写在**四份平行清单**里——`Settings` 的字段
（`config.rs:19-92`）、它的 `Default`（`:94-131`）、`PipelineOverrides` 的 `Option` 字段
（`:136-172`）、`apply` 里的 `set!` 宏参数表（`:183-215`）。精确数字：**32 / 32 / 31**。

**四份里只有宏清单是静默的**：字段加进两个 struct 却忘了加进宏清单 → 该键在 `config.toml` 里写了
**读回来还是默认值**，而**没有任何测试会红**（`defaults_match_design_table` 只断言 23 项、
`toml_override_only_touches_named_fields` 只断言 3 个字段；`PipelineOverrides` 全仓只有 `config.rs`
里 4 处引用）。另外三份漏写是**编译错误**。判据见词条「**清单同一性**」。

**为什么 `env_mode` 走特殊路径（且这是对的）**：`EnvMode` **有** `impl Deserialize`
（`types.rs:1403`，`#[serde(rename_all = "snake_case")]`），故 `Option<String>` 不是类型所迫——
理由是**报错质量**（`"Auto"` 走枚举会得到一句难读的 serde 错误，而这里要的是「只能是 auto /
ask / deny」），由 `the_global_tier_accepts_auto_and_deny_but_not_ask`（`config.rs:1266`）钉住文案。
**故它是豁免，不是漂移**（本轮评审报告曾把它写成漂移，见 README §六.3 的订正）。

**Blocked by:** None（可立即开始）

**Status:** done（已实现）

- [x] 加一条**定值探针**测试（不是键集比对——键集只防「字段集漂」，防不住「字段在清单里但值没被
      合上」）。形状：
      - 由 `serde_json::to_value(PipelineOverrides::default())` 取到字段名集合（`Settings` 无任何
        per-field serde 属性、`serde(default)` 不跳过字段，故 32 个键全在）；
      - 对每个字段**机械化**喂一个非默认值（数值 `default + 1`、布尔取反、`Vec` 追加一项、`Option`
        包的同上），反序列化成一个 `PipelineOverrides`；
      - 走 `apply(&Settings::default())`，断言结果**每一项都 ≠ 默认值**；
      - **判据必须遍历默认值对象、不能手写字段数组**（手写就又是抄一遍，而那正是本条要消灭的形状）。
- [x] **不设豁免表**——豁免表本身就是一份会漂的清单。`env_mode` 的探针写成**显式常量**
      （`"deny"`，必须是合法档位字面量）并加注释说明它走 `apply` 里的特殊路径、故不参与机械填值；
      注释承担「解释」，不承担「枚举」
- [x] 断言 `env_mode` **也**被合上（它的特殊路径**会**正常生效：`"deny"` → `EnvMode::Deny`）——
      即豁免的是**填值方式**，不是**覆盖关系**
- [x] 顺手断言字段数：`Settings` 序列化出的键数 == `PipelineOverrides` 的键数（32 == 32），
      这条抓「字段加进一边忘了另一边」
- [x] `docs/testing.md` L1 用例目录加 row（锚决策 258）
- [x] `docs/decisions.md` 追加**决策 258**（本批已落，核对即可）

**零行为变化**（纯加一条测试）。

**明确不做**：**不立设置注册表**（把 32×4 重写成声明式表示）——这不是同一件事，且当前没有一次真实
漂移需要它；不做代码生成；不引入豁免清单；不改 `env_mode` 的双层解析（`types::effective_env_mode`
仍是唯一实现，决策 206 不变）；不给 `ServerConfig` / `LoggingConfig` / `PromptsConfig` /
`SkillsConfig` / `MarketConfig` 做同样的探针（它们各自是单一定义 + `serde(default)`，没有平行清单
的失败模式——**若将来某个也长出覆盖层，照本票同一形状补**）。

---

## 实施收尾（2026-09-23）

`make check` 全绿。落点 `crates/core/src/config.rs` 尾部两条用例：

- `every_setting_field_is_actually_overridable`——字段集相等（32 == 32）+ 定值探针逐项断言。
  按 `Value` 的 variant 推类型（`Bool` 取反 / `Number` 给 `7` / `String` 加后缀 / `Array` 追加一项），
  新增类型会在 `panic!` 的报文里点名要人补一条。
- `env_mode_override_takes_effect_through_its_special_path`——豁免的是**填值方式**、不是覆盖关系：
  `"deny"` 走特殊路径**也要**合上（`EnvMode::Deny`），认不出的值保持 base 不猜。

**牙齿已验**：把 `max_concurrent_tasks` 从 `set!` 宏清单里拿掉（正是那个静默失败模式），
第一条当场红并**点名**那个字段：

```
这些字段喂了非默认值却仍等于默认值——它们没被 `set!` 宏清单接住：["max_concurrent_tasks"]
```

**探针的一处设计取舍（票面未写、实现时定的）**：`env_mode` 的探针断言里带了
`assert_eq!(current, &json!("auto"))`——即顺手钉住「它的默认是 `auto`」。这一条不是必须的
（探针只要求「≠ 默认」），但它把「有人把全局默认改成别的档位」也变成一个会红的点，
而那是一次影响全机所有阶段的编辑（`config.rs` 的字段 doc 明写这个立场），值得被看见。
