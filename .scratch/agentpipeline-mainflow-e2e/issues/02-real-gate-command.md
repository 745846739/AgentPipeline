# 02: 闸门真跑测试命令（fixture 换真实工程）

**What to build:** e2e fixture 仓库刻意不含任何语言标记文件（`harness.ts:242-251`），
于是 `detect_language` 返回 `None` → `detect_test_framework` 返回 `None` →
`test_command_for(None)` 返回 **`true`**（`crates/core/src/pipeline/executor.rs:3270`）。
结果是：**主流程里必然经过的闸门环节，在端到端路径上被短路成空操作。**

真实用户用的是 Rust / Node / Python 项目，闸门会真跑 `cargo test --quiet` /
`npm test --silent` / `python3 -m pytest -q`；命令超时（`test_command_timeout_sec` 默认 600s）、
输出解析、失败后按 `gate_failure_kind` 分流（lint → 直接打回 develop，test → 去 test.execute 复检）、
`gate_failures` 累加与耗尽——这一整条链在浏览器路径上一次都没跑过。

本票给 fixture 装一个**真实的小工程**并让闸门真的执行一次测试命令。

**Blocked by:** 01

**Status:** done（2026-09-13）

- [x] fixture 增加最小可构建工程并使其测试命令**快速真跑**（建议 Node 工程：只需
      `package.json` + 一个 `node --test` 或用现成零依赖脚本；**不要**选 Rust——编译耗时会让
      每条 e2e 不可接受地变慢，且 `make build` 已在跑 cargo）。选型须在票面实现时记录理由与实测耗时
- [x] fixture 的 `package.json` 带 `test` 脚本，使 `detect_test_framework` 探测为 `npm`
      → 闸门命令为 `npm test --silent`（或所选方案对应的真实命令）
- [x] 主流程用例断言闸门**真的执行了**：至少一项下界
      （`kanban_node_commands` 里出现该测试命令 / 命令输出非空 / merge 元数据 `gate` 有值且
      非「跳过」）——**仅断言任务到 done 不算**，因为命令为 `true` 时同样会 done
- [x] 新增一条「闸门失败」用例：让 fixture 的测试脚本**先失败**，断言
      （a）任务不停在 done；（b）`gate_failures` 累加；（c）按 `gate_failure_kind` 走了正确分流
      （test → test.execute 复检 / lint → 直接 develop，取决于所配命令）；
      然后修好测试脚本 → resume → 走到 done
- [x] 记录实测耗时，并在票面记下对 `just frontend-e2e` 总时长的影响；若单条超过 60s，
      在 `playwright.config.ts` 显式调整该用例 timeout 而非全局放宽
- [x] 全量闸门绿：`just lint` / `just test` / `just frontend-e2e`

**目标产物那侧：** 本票只保证闸门**真跑**。在契门上断言「合入的代码是否符合任务目标」由票 01
的产物断言承接，两票合起来才是完整主流程证据。
