# 02: 节点级技能声明 + executor 注入

**What to build:** 让技能可按**节点**声明：新增 `node_skills()` 读
`node_overrides_json[node].skills`，executor 把「阶段级 ∪ 节点级」合并后经 `skills::resolve`
解析成「名字 + 正文」注入 system prompt。解决阶段级 `skills_json` 无法区分节点的问题——
architect-design 的 validate_input 要拷问、execute 要写文件，阶段级注入会让二者互相打架。

**Blocked by:** 01（需要 `skills::resolve`）

**Status:** done（2026-09-14，决策 170）

- [x] `config.rs::node_skills(stage_cfg, node)` 读 `node_overrides_json[node].skills`，
      非数组 / 非字符串元素忽略（与 `json_string_list` 同宽严）
- [x] `executor.rs::agent_attempt`：阶段级 + 节点级合并 → `effective_skills`（并集、mandatory
      在前、去重）→ `skills::resolve(home.root(), ...)` → `build_system_prompt`
- [x] **只增不减**：节点级只能加技能，不能削减阶段级或 mandatory
      （`stage_level_skills_still_apply_and_union_with_node_level` 钉住）
- [x] 伪阶段两处调用点（`call_pseudo_stage` / `project_analysis`）保持 `&[]` 不动
- [x] `validate_startup`：阶段级与节点级声明过同一套检查；节点级报错**定位到节点**
      （`missing_node_skill_refuses_startup_with_node_in_message` 断言错误串含节点名）
- [x] L2 端到端证明（FakeAgent 记录的真实 `LlmRequest.system_prompt`）：
      `node_scoped_skills_inject_different_bodies_per_node` 断言 validate_input 的 prompt
      含 `### grilling` + `frontier` 且**不含** `### to-spec`，execute 反之；
      validate_output 未声明技能 → 无技能段
- [x] 阶段级 `skills_json` 旧行为不回归（并集用例同时覆盖 `- rtk` 与 `### grilling` 并存）

**Notes（实现结论）:**
- 节点级技能是**纯增量**能力：不写 `node_overrides_json` 的阶段行为与改动前逐字相同
  （阶段级技能、工具型技能渲染、prompt 段落顺序均未变）。
- 校验复用 `declared_skills(cfg)` 收集「定位说明 + 技能名」两组声明，阶段级与节点级共用
  同一条校验与同一条报错风格，不写第二套实现。
- **踩坑记录（给后续写 L2 用例的人）**：executor 的单执行者注册表
  （`EXECUTOR_REGISTRY`，决策 36）是**进程级**的、只按 `task_id` 去重。并行跑测试时若新用例
  复用既有 id（如 `t7` / `t8`），第二个 `run()` 会**静默 no-op 并返回 Ok**，表现为
  「request_log 为空」而非报错。新用例必须用唯一 id（本票用 `t-node-skills` /
  `t-skills-union`）。这是既有测试基建的既有特性，非本票引入。
