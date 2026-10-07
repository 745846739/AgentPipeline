# 01 阶段写入面白名单：FileToolPolicy 增正向白名单 + 按 ProductTarget 装配

Status: ready-for-agent
决策号预留: 395（落地时续写 docs/decisions.md，扩展决策 283 的操作环境域）

## 目标

把 grilling Q7=C 统一规则落进代码：**任务目录的 agent 写入一律白名单到该节点
`ProductTarget` 声明的产出文件；worktree 写入仅 develop 与 test 放开。**

## 改动一：`FileToolPolicy` 增正向白名单字段

`crates/core/src/agent/file_policy.rs:21-31` 的 `FileToolPolicy` 现状是
「允许根（worktree + 任务目录）+ `deny_paths` 拒绝名单」，表达不了
「只许写这一个文件」。新增：

```rust
/// 非空时：写操作必须命中其中一条才放行；空 = 不限制（向后兼容旧调用点）
pub allow_writes: Vec<WriteAllow>,

#[derive(Clone, Debug)]
pub struct WriteAllow {
    pub root: WriteRoot,      // Worktree | TaskDir
    pub pattern: String,      // 相对该根的 glob（本票只用到精确文件名与 "**"）
}
```

判定顺序保持「deny 优先」不变，在现 `check()`（`file_policy.rs:126-173`）的
允许根命中之后追加：`allow_writes` 非空时，写 op 解析出的 `(root, 相对路径)`
必须命中至少一条（glob 用现成 crate，与 `deny_paths` 同风格）；不命中 →
`Error::PolicyDenied`，报错信息带「本阶段允许写入面」清单（供回灌 prompt 自纠）。

注意：白名单判定要在 `write_root_for`（`tools.rs:1196, 1216, 1313`）解析出
目标根**之后**做，防「相对路径拼 A 根、绝对路径写 B 根」绕过——`check_write`
需要同时拿到 root 与 realpath 后的最终路径。

## 改动二：按 stage/node 装配

装配点两处（每次 attempt 构造 policy 的地方）：

- `crates/core/src/pipeline/model_invoke.rs:769-773`（`cursor.stage` 在作用域）
- `crates/core/src/pipeline/subagent.rs:214`（子代理同款）

新增装配函数（放 `file_policy.rs`）：

| stage | allow_writes |
|---|---|
| ArchitectDesign / DevelopDesign / TestDesign / Review | `[TaskDir <ProductTarget 文件名>]` |
| Test | `[Worktree "**", TaskDir "test-report.md"]` |
| Develop | `[Worktree "**"]`（任务目录零条目 = 拒写） |

白名单值**直接调 `continuation_brief.rs:66-89` 的 `product_target`** 取
`File(name)`，不另立第四处文件名清单。既有「三处同源」注释
（`continuation_brief.rs:76-78`）更新为「四处同源、以 `product_target` 表为准」；
不重构旧三处（不扩本票 scope）。

## 改动三：模板交叉引用

三个受影响模板补一句白名单告知（照 test-design VO 模板
「引用悬空会被 sync-check 机械校验拦下」的既有写法，
`templates.rs:226`）：「本阶段的写入面已被文件策略收紧为 <产出文件>，
越界写入会被拒绝并回灌」。

## 断言清单（四件套之「钉死」）

单测（`file_policy.rs` 内，照 symlink 逃逸测试 `file_policy.rs:469-481` 的形状）：

1. review 写 `review-report.md` → 放行；写任务目录其他文件 → `PolicyDenied`；
   写 worktree → `PolicyDenied`（`write_root_for` 对 Review 返回 task_dir，
   需用绝对路径构造 worktree 写入来验证不被根混淆放过）。
2. develop 写任务目录任意路径 → `PolicyDenied`；写 worktree → 放行。
3. test 写 `test-report.md` → 放行；写任务目录其他文件 → 拒；写 worktree → 放行。
4. `allow_writes` 为空的旧形状 policy 行为与今日完全一致（向后兼容）。
5. 白名单与 `deny_paths` 冲突时 deny 优先（deny 顺序不变量）。
6. 装配测试：`file_policy_for_stage` 对六阶段输出与上表逐行相等，
   且值来自 `product_target`（改表自动联动，防漂移）。

e2e（`tests/e2e/tests/integration/` 下新文件或并入 `gates.rs`）：

7. mock review agent 越界 write_file → 断言 `PolicyDenied` 回灌、最终产出不受污染、
   transition 台账可见拒绝记录。
8. mock develop agent 写任务目录 → 断言拒绝且不影响 Retry 计数语义
   （策略拒绝是否计 attempt：**不计**，照 metadata 校验失败的回灌先例，
   `agent/metadata.rs` 模块头）。

## 回归保护

`docs/testing.md` 用例目录补两行（e2e-内容边界-01/02），指向上述断言。
