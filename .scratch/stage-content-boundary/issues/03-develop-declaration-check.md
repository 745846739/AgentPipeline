# 03 develop 申报单向比对：changed_files 申报 vs worktree 实际 diff

Status: ready-for-agent
决策号预留: 397（落地时续写 docs/decisions.md，扩展决策 391 的 develop 闸门
判据——形状同零提交守卫；不触决策 85 的失败分类）

## 目标

grilling Q9=B + Q10=A + Q11=C：review 的变更视图完全信任 develop 申报的
`changed_files`/`unit_test_files`（`model_request.rs:538-541`），漏报即盲审。
在 develop 自己的纯代码闸门里做机械比对，当轮拦下、当轮自愈。

## 改动一：比对检查进 develop_code_gate

落点 `crates/core/src/pipeline/executor.rs:877-944`（`develop_code_gate`，
零提交守卫已在其中，`executor.rs:868-876`）。新增第四项检查：

1. diff 面：与零提交守卫同基准——任务分支相对基准分支
   `git diff --name-only <base>...HEAD`（系统侧 `run_system_command` 跑，
   不占 agent 权限）。
2. 申报面：`stage_output_metadata(Stage::Develop, OUTPUT_CODE_CHANGES)`
   的 `changed_files ∪ unit_test_files`（路径归一化：剥 `./`、统一分隔符）。
3. 判定（**单向只查漏报**，Q11）：diff 里有、申报里没有、且不命中噪音过滤
   的文件存在 → 检查失败。申报里多报的条目**容忍**（多报无害，双向严格会
   被 lockfile 漂移打脸）。
4. 申报元数据缺失 → 视为全漏，失败（fail-closed，对齐决策 370 的
   `metadata_gaps` 语义）。

## 改动二：噪音过滤

配置项（`config.rs`，任务级可覆盖、全局缺省）：

```toml
[gate]
declare_ignore_globs = ["**/Cargo.lock", "**/package-lock.json",
                        "**/pnpm-lock.yaml", "**/yarn.lock", "**/poetry.lock",
                        "**/*.sum", "**/.DS_Store"]
```

- glob 匹配复用 01 票引入的同一 matcher（不引第二套 glob 依赖）。
- 命中过滤的文件不计漏报，但**落 facts**：追加进
  `zero-commit-facts.md` 同目录的既有 facts 文件（或改名
  `develop-gate-facts.md`，实施时定，保持决策 391 的消费点同步），
  记「已忽略的未申报文件」，供审计区分「干净漏报零」与「有噪音被滤」。

## 改动三：失败路由（Q10=A）

失败同 lint/单测失败走既有 Retry 回灌（`routes.rs` 的 develop 分支无新分支）：

- 回灌信息必须携带**具体漏报清单**（文件名 + 提示「补申报或撤销变更」），
  照零提交守卫写 facts 的形状（`executor.rs:868-876`）。
- **不计新增 `GateFailureKind`、不动 merge 路由**：比对失败在制造点
  （develop.validate_output）当轮暴露，不存在决策 391 `EmptyBranch` 那种
  「穿越四阶段才被发现」的改道需求；attempts 耗尽自然落既有
  `pending(retry_exhausted)` 收口。

## 断言清单

单测（executor 层，构造 worktree + 假 diff）：

1. diff 与申报一致 → 过。
2. diff 多一个未申报文件 → 失败，错误信息含该文件名。
3. 未申报文件命中 `declare_ignore_globs` → 过，facts 有忽略记录。
4. 申报了但 diff 没有 → 过（单向不变量）。
5. 申报元数据缺失/空 → 失败（fail-closed）。
6. 路径归一化：`./src/lib.rs` vs `src/lib.rs` 视为已申报。

e2e（`tests/e2e/tests/integration/gates.rs`，照「测试闸门失败不得直接打回
develop」断言 `gates.rs:100-105` 的写法）：

7. mock develop 漏报一个文件 → 断言 develop 重试、回灌 prompt 含漏报名单、
   补申报后过、**无** `GateFailureKind` 落库、merge 路由未被触碰。
8. lockfile 漂移未申报 → 断言一次过（噪音不假红——106 实证假红空转的教训）。

## 边界与不做

- 不做「申报 vs diff 双向严格相等」（Q11 已裁）。
- 不做语义比对（申报内容与 diff 内容是否相符是 review 的活，
  对齐决策 370 对 sync-check 的划界：纯代码节点不做语义）。
- 比对仅对 `CodeChanges` 申报面；`no_changes=true` 的申报由零提交守卫
  既有逻辑处理，本票不改其语义（`no_changes=true` 且 diff 非空仍是
  零提交守卫的失败，先于本检查短路）。
