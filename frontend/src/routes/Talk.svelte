<script lang="ts">
  import type { AllowedAction, BranchCursor, TaskListItem } from '../api/types';
  import { getConversations, getTask } from '../api/client';
  import { board } from '../stores/board.svelte';
  import {
    formatDuration,
    formatTokens,
    pendingLabel,
    taskDuration,
  } from '../lib/pipeline';
  import Sprite from '../components/render/Sprite.svelte';
  import Gauge from '../components/render/Gauge.svelte';
  import PendingActions from '../components/board/PendingActions.svelte';
  import { router } from '../router.svelte';

  /**
   * 对讲台（theme-6-pixel.md §3.3「与工头对话」；决策 174）。
   *
   * **不造聊天组件**：一列对话 = 一叠操作台对话框（`.turn` 即对话框本体：
   * 双线框 + 压在框沿上的名牌 tab = 发言者），故不再另加一行 who，也不做左右交替气泡。全站唯一的响仍只在
   * 急停一处——待拍板那轮挂 `.warn`（琥珀框 + ▼ + 恢复动作），其余轮次一律收在 `--pane`。
   *
   * **数据全部真实，不编造对话内容**：本页把**已有端点**的读数组织成对话形态——
   * 工头的发言 = 由 pending 理由 / 各工位会话摘要**确定性拼装**的状态转述（不是 LLM 生成）；
   * 工位回执 = 各 stage 的会话（`GET /tasks/{id}/conversations`）与命令流。没有真实
   * agent 人格之前，工头不是一个会说话的后端实体（决策 50 / 附录 B.2 属 v2），
   * 故此页**是真实状态的对话式视图，不是可自由对话的 chat**——输入口只承载
   * `requires_input` 的恢复动作（决策 79 的唯一自由输入例外）。
   */

  let loading = $state(true);
  let error = $state<string | null>(null);
  /** 每个 pending 任务的详情（allowed_actions 只在详情里下发，决策 101）。 */
  let details = $state<
    Record<string, { actions: AllowedAction[]; cursors: BranchCursor[] }>
  >({});
  /** 每个 pending 任务的节点运行摘要（工位回执的真实读数）。 */
  let conversationsByTask = $state<
    Record<string, Awaited<ReturnType<typeof getConversations>>>
  >({});

  const pending = $derived(board.pendingTasks);
  const running = $derived(board.tasks.filter((t) => t.status === 'running'));

  /** 8 工位的值班灯：按列聚合，与看板列头同一套状态语汇。 */
  const crew = $derived(
    [
      { key: 'init', label: 'init', sprite: 'flag' as const },
      { key: 'architect-design', label: 'architect-design', sprite: 'gem' as const },
      { key: 'develop-design ∥ test-design', label: 'develop-design ∥ test-design', sprite: 'hammer' as const },
      { key: 'develop', label: 'develop', sprite: 'gear' as const },
      { key: 'review', label: 'review', sprite: 'lens' as const },
      { key: 'test', label: 'test', sprite: 'shield' as const },
      { key: 'merge', label: 'merge', sprite: 'merge' as const },
      { key: 'done', label: 'done', sprite: 'trophy' as const },
    ].map((col) => {
      const stages =
        col.key === 'develop-design ∥ test-design'
          ? ['develop-design', 'test-design']
          : [col.key];
      const tasks = board.tasks.filter((t) => stages.includes(t.current_stage));
      const pen = tasks.some((t) => t.status === 'pending');
      const live = tasks.some((t) => t.status === 'running');
      const don = tasks.length > 0 && tasks.every((t) => t.status === 'done');
      return {
        ...col,
        count: tasks.length,
        state: pen ? 'warn' : live ? 'run' : don ? 'done' : 'idle',
      };
    }),
  );

  /** 工头开场白：由真实计数确定性拼装（不是模型生成的寒暄）。 */
  const briefing = $derived.by(() => {
    const p = pending.length;
    const r = running.length;
    if (p === 0 && r === 0) {
      return '夜班安静。当前没有在跑的任务，也没有等你拍板的事。';
    }
    const bits: string[] = [];
    if (r > 0) bits.push(`${r} 个在跑`);
    if (p > 0) bits.push(`${p} 个急停等人`);
    return `夜班正常。${bits.join('、')}。`;
  });

  /** 最久的 pending（等最长的在最上，与移动款待处理页同一口径）。 */
  const oldest = $derived(
    [...pending].sort((a, b) => Date.parse(a.updated_at) - Date.parse(b.updated_at))[0],
  );

  /**
   * 拉取每个 pending 任务的详情与会话摘要。
   *
   * **必须随 pending 集合变化重拉，不能只在 onMount 拉一次**：`board.init()` 是异步的
   * （App.svelte 的 onMount 发起），本页 onMount 时 `board.tasks` 往往还是空的——
   * 只拉一次的话 `pending` 为空、`details` 永远为空，页面会安静地退化成「打开任务详情」
   * 按钮，把后端下发的恢复动作整片吞掉（e2e ⑩ 打红即此）。
   */
  async function loadDetails(tasks: TaskListItem[]) {
    loading = true;
    error = null;
    try {
      const next: Record<string, { actions: AllowedAction[]; cursors: BranchCursor[] }> = {};
      const convs: Record<string, Awaited<ReturnType<typeof getConversations>>> = {};
      await Promise.all(
        tasks.map(async (t) => {
          try {
            // allowed_actions 只在详情下发（决策 101）；会话摘要作工位回执读数
            const [detail, list] = await Promise.all([getTask(t.id), getConversations(t.id)]);
            next[t.id] = { actions: detail.allowed_actions, cursors: detail.cursors };
            convs[t.id] = list;
          } catch {
            // 单个任务失败不影响整页（与看板补详情同一姿态）
          }
        }),
      );
      details = next;
      conversationsByTask = convs;
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  /**
   * 待办集合的指纹：只含 id 与 pending 类型。用它驱动重拉——
   * 集合不变时不重拉（避免 $effect 自激），集合一变（board 装载完成 / resume 后状态翻转）
   * 才重取。类型也进来是因为 `pending_updated` 会换理由而 id 不变。
   */
  const pendingKey = $derived(
    pending
      .map((t) => `${t.id}:${t.pending_reason?.type ?? ''}:${t.current_stage}`)
      .sort()
      .join('|'),
  );

  let lastKey = $state<string | null>(null);
  $effect(() => {
    const key = pendingKey;
    if (key === lastKey) return;
    lastKey = key;
    // 空集合无需拉取：直接清空上一次的读数
    if (!key) {
      details = {};
      conversationsByTask = {};
      loading = false;
      return;
    }
    void loadDetails(pending);
  });

  async function handleAction(
    taskId: string,
    action: AllowedAction,
    opts: { cursorId?: string; input?: string },
  ) {
    // board 重载后 pending 集合会变，$effect 里的指纹驱动重拉详情；
    // 这里显式再拉一次是为了动作回执后立刻反映（不等下次轮询）。
    await board.handleTaskAction(taskId, action, opts);
    await loadDetails(board.pendingTasks);
  }
</script>

<div class="talk">
  <div class="talk-head">
    <h1 class="tt">对讲台</h1>
    <div class="ts">
      <span>夜班态势：8 工位</span>
      <span class="sep">▪</span>
      <a class="crumb" href="#/" onclick={() => router.navigate('/')}>看板</a>
    </div>
  </div>

  {#if error}
    <div class="blank error">{error}</div>
  {/if}

  <main class="talk-main">
    <!-- 工头开场：由真实计数拼装的状态转述 -->
    <div class="turn">
      <div class="dname">工头</div>
      <p>{briefing}</p>
      {#if oldest}
        <p>
          <b>最久的一处</b>是「{oldest.title}」，
          {pendingLabel(oldest.pending_reason)}，停在 {oldest.current_stage}。
        </p>
      {/if}
    </div>

    {#if loading && pending.length === 0}
      <div class="turn">
        <div class="dname">工头</div>
        <p>正在读夜班台账…</p>
      </div>
    {:else if pending.length === 0 && running.length === 0}
      <!-- 全空态：开场白已报了「没有待办」，此处不再重复一句，只给下一步 -->
      <div class="turn">
        <div class="dname">操作台</div>
        <p>
          新建一个任务，流水线会从 init 开始走；走到需要你拍板的地方，这里会出现操作台对话框。
        </p>
        <button type="button" class="btn solid" onclick={() => router.navigate('/')}>
          去看板新建任务
        </button>
      </div>
    {:else}
      <!-- 每个 pending 任务 = 一轮「等你拍板」的对话框（全站唯一"响"的一处） -->
      {#each pending as task (task.id)}
        {@const detail = details[task.id]}
        {@const convs = conversationsByTask[task.id] ?? []}
        <div class="turn warn">
          <div class="dtag">⏸ 等你拍板 · {pendingLabel(task.pending_reason)}</div>
          <div class="dname">工头</div>
          <p>
            「{task.title}」走到 {task.current_stage}，{task.pending_reason?.message ?? '需要你决定'}。
          </p>
          <div class="ctx">
            状态：<b>{task.status}</b> ▪ 已跑 {formatDuration(taskDuration(task))} ▪
            <Gauge tokens={task.total_tokens} tone="warn" /> {formatTokens(task.total_tokens)} tok
          </div>

          <!-- 回执：转述各工位读数（真实会话摘要），左缘亮度阶、无框 -->
          {#if convs.length > 0}
            <details class="rcpts">
              <summary class="rcpts-sum">
                工位回执 <span class="dim">{convs.length} 次节点运行 ▸</span>
              </summary>
              {#each convs.slice(-6) as c (c.run_id)}
                <div class="rcpt">
                  <div class="rcpt-head">
                    <Sprite name="gear" size={10} />
                    <span class="nm">{c.stage}</span>
                    <span class="dim">{c.agent_type}</span>
                    <span class="rs">{formatTokens(c.prompt_tokens + c.completion_tokens)} tok</span>
                  </div>
                </div>
              {/each}
            </details>
          {/if}

          <div class="grp">恢复动作</div>
          {#if detail}
            <PendingActions
              actions={detail.actions}
              cursors={detail.cursors}
              pendingType={task.pending_reason?.type}
              disabled={board.actionBusy === `${task.id}:${task.pending_reason?.type}`}
              onaction={(a, opts) => handleAction(task.id, a, opts)}
            />
          {:else}
            <button type="button" class="btn" onclick={() => router.navigate(`/task/${task.id}`)}>
              打开任务详情
            </button>
          {/if}

          {#if board.actionError && board.actionBusy === null}
            <div class="ctx err">{board.actionError}</div>
          {/if}
        </div>
      {/each}

      {#if running.length > 0}
        <!-- 在跑的工位：工头报一句进度（真实读数），不占恢复动作 -->
        <div class="turn">
          <div class="dname">工头</div>
          <p>
            还有 {running.length} 个在跑：{running.map((t) => t.title).join('、')}。
            它们会自己往下走，走完再叫你。
          </p>
          {#each running.slice(0, 4) as t (t.id)}
            <div class="rcpt">
              <div class="rcpt-head">
                <Sprite name="gear" size={10} />
                <span class="nm">{t.title}</span>
                <span class="rs live">▶ {t.current_stage}</span>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    {/if}

    <!-- 操作台：本页没有自由对话（决策 79 唯一自由输入是 info_insufficient 的补充说明，
         已在各轮的恢复动作里）。此处只做指引，不做假输入框。 -->
    <div class="typer">
      <div class="dname">操作台</div>
      <p class="typer-note">
        自由对话需要一个会说话的工头 agent——决策 50 / 附录 B.2 把它排在 v2，
        v1 的输入口只承载「信息不足」的补充说明。上面每一轮的恢复动作就是当前可下发的全部动作。
      </p>
    </div>
  </main>

  <!-- 右栏：值班板（复用台账盒语汇） -->
  <aside class="talk-side">
    <div class="reg">
      <div class="reg-head"><span>值班板</span><span class="n">8 工位</span></div>
      <ul class="brows">
        {#each crew as c (c.key)}
          <li class="brow {c.state === 'warn' ? 'pen' : c.state === 'run' ? 'hot' : ''}">
            <span class="blamp {c.state === 'warn' ? 'w' : c.state === 'run' ? 'c' : c.state === 'done' ? 'd' : ''}"></span>
            <span class="bnm">{c.label}</span>
            <span class="bc">{c.count}</span>
          </li>
        {/each}
      </ul>
      <div class="boks">
        在跑的工位会自己往下走，不用追问。<br />
        急停的只能你来按键。
      </div>    </div>
  </aside>
</div>

<style>
  /* 布局与视觉逐条对齐 theme-6-pixel.md §3.3 / 原型 #v-talk */
  .talk {
    max-width: var(--split-max, 1240px);
    margin: 0 auto;
    padding: 18px 20px 44px;
    display: grid;
    grid-template-columns: 1fr var(--dossier-w, 340px);
    gap: 18px;
    align-items: start;
  }
  .talk-head {
    grid-column: 1 / -1;
    display: flex;
    align-items: baseline;
    gap: 14px;
    flex-wrap: wrap;
  }
  .tt {
    font-size: 24px;
    font-weight: 400;
    color: var(--text-hi);
    line-height: 1.2;
  }
  .ts {
    display: flex;
    gap: 14px;
    color: var(--text-3);
    align-items: baseline;
  }
  .sep {
    color: var(--text-4);
  }
  .talk-main {
    min-width: 0;
  }
  .talk-side {
    position: sticky;
    top: 86px;
  }

  /* ── 单轮发言：操作台对话框本体（双线框 + 名牌 tab 即发言者） ──
     `.dname` 名牌在 PendingDossier 里是局部样式、未进 app.css，故本页自带一份，
     取值与 §3.3 / 原型逐字一致。 */
  .turn {
    position: relative;
    margin: 26px 0 0;
    padding: 10px 12px 11px;
    background: var(--bg);
    border: 2px solid var(--pane);
    box-shadow:
      inset 0 0 0 2px var(--bg),
      inset 0 0 0 4px var(--pane);
  }
  .turn p {
    color: var(--text);
    margin-bottom: 7px;
    max-width: 76ch;
    overflow-wrap: anywhere;
  }
  .turn p:last-child {
    margin-bottom: 0;
  }
  .dname {
    position: absolute;
    top: -16px;
    left: 6px;
    background: var(--bg);
    border: 2px solid var(--pane);
    color: var(--text-hi);
    padding: 0 8px;
    line-height: 1.5;
    white-space: nowrap;
  }
  /* ▼ 光标默认不画：只有待拍板那一轮点亮（§3.3 纪律 2） */
  .turn::after {
    content: none;
  }
  .turn.warn {
    border-color: var(--pending);
    box-shadow:
      inset 0 0 0 2px var(--bg),
      inset 0 0 0 4px var(--pane),
      4px 4px 0 var(--ink);
  }
  .turn.warn .dname {
    border-color: var(--pending);
    color: var(--pending);
  }
  .turn.warn::after {
    content: '▼';
    position: absolute;
    right: 6px;
    bottom: 0;
    color: var(--pending);
    font-size: 12px;
    line-height: 1;
    animation: blink 1s steps(2) infinite;
  }
  .dtag {
    color: var(--pending);
    margin-bottom: 8px;
  }
  .ctx {
    color: var(--text-2);
    margin-bottom: 6px;
    line-height: 1.8;
    overflow-wrap: anywhere;
  }
  .ctx b {
    color: var(--go);
  }
  .ctx.err {
    color: var(--stop);
  }
  .grp {
    font-size: 12px;
    color: var(--text-4);
    letter-spacing: 0.08em;
    margin: 12px 0 7px;
  }
  .dim {
    color: var(--text-4);
  }

  /* ── 工位回执：转述不是发言（左缘 4px 亮度阶 + 无框，与命令输出同一手法） ── */
  .rcpts {
    margin-top: 8px;
  }
  .rcpts-sum {
    color: var(--text-3);
    cursor: pointer;
    list-style: none;
  }
  .rcpts-sum::-webkit-details-marker {
    display: none;
  }
  .rcpt {
    border-left: 4px solid var(--pane);
    background: var(--panel);
    padding: 6px 10px;
    margin-top: 6px;
  }
  .rcpt-head {
    display: flex;
    align-items: center;
    gap: 7px;
    color: var(--text-3);
  }
  .rcpt-head .nm {
    color: var(--text-2);
  }
  .rcpt-head .rs {
    margin-left: auto;
    flex: none;
    color: var(--text-4);
  }
  .rcpt-head .rs.live {
    color: var(--go);
  }

  /* ── 操作台：不做假输入框，只讲清 v1 的边界 ── */
  .typer {
    position: relative;
    margin: 28px 0 0;
    padding: 12px 12px 10px;
    background: var(--bg);
    border: 2px solid var(--pane);
    box-shadow:
      inset 0 0 0 2px var(--bg),
      inset 0 0 0 4px var(--pane);
  }
  .typer .dname {
    color: var(--text-3);
  }
  .typer-note {
    color: var(--text-3);
    line-height: 1.8;
    max-width: 78ch;
  }

  /* ── 值班板（复用 .reg 台账盒语汇） ── */
  .brows {
    list-style: none;
  }
  .brow {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 5px 12px;
    border-bottom: 2px solid var(--wash);
    color: var(--text-3);
  }
  .brow:last-child {
    border-bottom: 0;
  }
  .blamp {
    flex: none;
    width: 8px;
    height: 8px;
    background: transparent;
    border: 2px solid var(--text-4);
  }
  .blamp.c {
    background: var(--go);
    border-color: var(--go);
  }
  .blamp.w {
    background: var(--pending);
    border-color: var(--pending);
  }
  .blamp.d {
    background: var(--done);
    border-color: var(--done);
  }
  .brow .bnm {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .brow.hot .bnm {
    color: var(--text-hi);
  }
  .brow.pen .bnm {
    color: var(--pending);
  }
  .brow .bc {
    flex: none;
    color: var(--text-4);
    font-variant-numeric: tabular-nums;
  }
  .boks {
    padding: 10px 12px;
    color: var(--text-3);
    line-height: 1.9;
  }

  .blank {
    padding: 10px 12px;
    border: 2px solid var(--pane);
    color: var(--text-3);
    margin-top: 10px;
  }
  .blank.error {
    border-color: var(--stop);
    color: var(--stop);
  }

  /* 移动款（§5）：右栏值班板收进正文流，单列 */
  @media (max-width: 479px) {
    .talk {
      display: block;
      padding: 12px 12px 30px;
    }
    .talk-side {
      position: static;
      margin-top: 18px;
    }
    .turn p {
      max-width: none;
    }
  }
</style>
