# 02: commands 移出首屏关键路径 + preview 字节兜底

**What to build:** 两件事，都打在同一个点上——1.33 MB 的 commands 载荷不该挡着时间线。

① **commands 移出首屏**。现在 `load()` 是 `await getTask` → `await Promise.all([getFlow,
getConversations, getCommands])` → **三个都回来才一次性赋值**（`frontend/src/stores/
taskDetail.svelte.ts:126-135`），于是时间线要的 `transitions`（来自只有 7 KB 的 `/flow`）
被 1.33 MB 的 commands 扣住。改成**不阻塞首屏 + 后台补拉**：`getCommands` 从 `Promise.all`
里挪出，首屏只等 `flow` 与 `conversations`；commands 在后台并发拉到后并入 `state.commands`。
**因此不新增计数端点**——页签徽标（`frontend/src/routes/TaskDetail.svelte:534`）沿用补拉
结果（初始短暂显示 0，随后更新）。

② **preview 字节兜底**。现在 preview 按**行数**截（`head_tail(&out.stdout, 50, 100)`，
`crates/core/src/agent/tools.rs:2304`），遇到超长单行就失控——本机库实测**单条 preview
最大 183,241 字符**，一个任务 preview 合计 2.4 MB，是该端点载荷的主体。保留行数口径
（「看头看尾」的原意），**加 4 KB 字节上限**，超出截断并留标记。完整输出本来就有卸载文件
与展开读取这条路（`GET /commands/{id}/output`）。

**Blocked by:** 04（**已满足**：闸门已落地并做过牙齿检查——见本票「落地」）

**Status:** done（2026-10-01，决策 361；106 验收待部署后补做）

## 落点

- `frontend/src/stores/taskDetail.svelte.ts`：`load()` 的 `Promise.all` 拆两段；
  commands 的补拉与并入（注意静默 refetch 路径 `:210-216` 不要重复拉）；
  `resetTaskContent()` / `dispose()` 对应处理。
- `crates/core/src/agent/tools.rs`：`head_tail` 的调用点加字节上限（或给 `head_tail`
  本身加一个 `max_bytes` 参数），并保证截断标记与既有形态一致。
- 若 preview 的落库点在别处（`crates/core/src/storage/observability.rs` 的
  `record_command_finish`），截断发生在**写入前**，别只截读取时的返回值。

## 验收

- [ ] 首屏（默认 `timeline` 页签）**不再等待 `/commands`**：Playwright 里拦截
      `/commands` 并挂住不返回，断言时间线内容仍渲染出来（确定性判据，见票 04）
- [ ] commands 仍会到：挂住的请求放行后，现场页签的命令回执与页签徽标数字正确
- [ ] 静默 refetch（SSE 驱动）不重复拉 commands，也不把已到的 commands 清空
- [ ] preview 字节上限生效：造一条含超长单行的命令输出，落库后的 `stdout_preview`
      ≤ 4 KB 且带截断标记；既有 command 相关测试全绿
- [ ] `make api` 契约测试全绿；`npm test` / `npm run check` 全绿

**明确不做**：把 preview 改成纯字节截断（丢掉「看头看尾」的语义）；给 commands 加
`stage`/`node` 服务端过滤（端点已支持 `?stage=&node=`，但那是「按需拉」的另一条路，
本批先做「全量但移出首屏」）。

**来源：** `.scratch/scene-read-path-perf/spec.md` 决议 2；首屏时序见
`frontend/src/stores/taskDetail.svelte.ts:113-135`；preview 截断见
`crates/core/src/agent/tools.rs:2304`。

## 落地

- `frontend/src/stores/taskDetail.svelte.ts`：`load()` 的 `Promise.all` 只留 `getFlow` +
  `getConversations`；commands 由新的 `loadCommands()` **后台补拉**后并入 `state.commands`。
  **只在非静默装载里发**（静默 refetch 不重拉，也不清已到的那一份——SSE 的 `command_started` /
  `command_finished` 与 `liveTools` 已承担在飞与增量，决策 359）；换过任务就丢掉回写（`this.id !== taskId`）。
  不新增计数端点，页签徽标沿用补拉结果。
- preview 字节兜底（`crates/core/src/agent/tools.rs`）：新增 `COMMAND_PREVIEW_MAX_BYTES = 4 * 1024`、
  `preview_text()` / `truncate_bytes()` / `take_bytes()`；`finish_command_output` 的 stdout / stderr
  两处改用它。行数与字节**两级并存**（只看字节会把「看头看尾」压成「只看头」，只看行数挡不住单行巨物）；
  截断在**写入前**（`CommandFinish` 就是落库的输入）。
- 测试：`tools.rs::preview_caps_bytes_but_keeps_the_head_tail_shape`；
  `frontend/src/stores/taskDetail.test.ts` 三条（commands 挂住不返回时 `transitions` 照样就位 /
  补拉失败挂错误位但不翻掉首屏 / 静默 refetch 不重拉不清空）；
  `frontend/e2e/first-paint-budget.spec.ts` 两条（真后端）。
- **牙齿检查**：把 `getCommands` 挪回 `Promise.all` → e2e 红（实测 31.9 s 超时失败），恢复后绿。
