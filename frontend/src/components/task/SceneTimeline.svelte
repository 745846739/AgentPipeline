<script lang="ts">
  import { tick } from 'svelte';
  import type { ConversationSummary, NodeCommand, NodeConversation } from '../../api/types';
  import type { LiveDelta, LiveTool } from '../../realtime/reduce';
  import { formatClock, formatTokens } from '../../lib/format';
  import { formatDuration } from '../../lib/pipeline';
  import { thinkTicker } from '../../lib/talkTurns';
  import { DEFAULT_PAGE, windowSlice } from '../../lib/windowSlice';
  import {
    buildTaskScene,
    sceneTurnMatches,
    type SceneTurn,
  } from '../../lib/taskScene';
  import MarkdownView from '../render/MarkdownView.svelte';
  import MetadataCard from '../render/MetadataCard.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import MoreRow from '../ui/MoreRow.svelte';

  /**
   * 任务详情「现场」页签（决策 349）：会话与命令输出合并成的**一条时间线**，
   * 版式取对讲台的那一套——一叠操作台对话框（`.turn` 即对话框本体，压在框沿上的
   * 名牌 = 发言者），过程按发生顺序排在轮里，命令是一枚带退出码的回执。
   *
   * 归约判断全部住在 `lib/taskScene.ts`（命令归哪一轮、消息怎么折步骤、流式增量接到
   * 哪一头上、直播流怎么交织折步、多次尝试谁主谁次、轮首的 prompt 与落地思考——决策
   * 359 / 360）；这里只接线：关键词过滤、窗口化（决策 319 的口径原样——先过滤后切）、
   * 展开态受控（与 Talk 同一手法：`preventDefault` 掉默认翻转，状态说了算）、思考步 /
   * 阶段 prompt / 旧一代尝试的折叠、流式期间的贴底跟随（只在人本就在底上时跟）。
   */
  interface Props {
    conversations: ConversationSummary[];
    conversationFor: (runId: number) => NodeConversation | undefined;
    commands: NodeCommand[];
    liveDeltas?: LiveDelta[];
    liveTools?: LiveTool[];
    /** 完整 / 流式输出解析（完整 > 流式；preview 由归约兜底）。 */
    commandOutputFor: (c: NodeCommand) => string | null;
    commandErrorFor?: (c: NodeCommand) => string | null;
    /** 请求加载完整输出 `GET /commands/{id}/output`（展开时没有才发）。 */
    onloadCommand?: (commandId: number) => void;
    /** 深链 / 跳转要带到眼前的那一轮（`?run=` 与档案盒的「去看对话」）。 */
    highlightRunId?: number | null;
  }

  let {
    conversations,
    conversationFor,
    commands,
    liveDeltas = [],
    liveTools = [],
    commandOutputFor,
    commandErrorFor,
    onloadCommand,
    highlightRunId = null,
  }: Props = $props();

  /** 关键词过滤（轮级，判据在 `sceneTurnMatches`）；过滤态不进 URL（决策 319 口径）。 */
  let query = $state('');
  let shownTurns = $state(DEFAULT_PAGE);

  const turns = $derived(
    buildTaskScene({
      conversations,
      conversationFor,
      commands,
      liveDeltas,
      liveTools,
      commandOutputFor,
      commandErrorFor,
    }),
  );
  const filtered = $derived(turns.filter((t) => sceneTurnMatches(t, query)));
  const turnSlice = $derived(windowSlice(filtered, shownTurns, 'tail'));

  // 换过滤词：窗口游标回缺省——省略计数是按当前名单算的，旧游标只会有害（决策 319）。
  $effect(() => {
    query;
    shownTurns = DEFAULT_PAGE;
  });

  /** 展开态（受控）：命令回执按命令 id、工具回执 / 思考 / prompt 按步骤键，各自一张表。 */
  let expandedCmd = $state<number | null>(null);
  let loadingCmd = $state<number | null>(null);
  let toolOpen = $state<Record<string, boolean>>({});
  let thinkOpen = $state<Record<string, boolean>>({});
  let promptOpen = $state<Record<string, boolean>>({});
  /** 每轮步骤切片的窗口游标（换 run 不重置：键随轮稳定，旧游标无有害 side effect）。 */
  let turnPages = $state<Record<string, number>>({});
  /** 台账命令按 id 的索引：回执展开时要拿**原命令**去问输出账与发加载（归约只留了显示字段）。 */
  const commandById = $derived.by(() => {
    const m = new Map<number, NodeCommand>();
    for (const c of commands) m.set(c.id, c);
    return m;
  });

  function toggleCmd(e: MouseEvent, id: number) {
    e.preventDefault();
    const cmd = commandById.get(id);
    if (!cmd) return;
    if (expandedCmd === id) {
      expandedCmd = null;
      return;
    }
    expandedCmd = id;
    // 与旧命令表同一判据：完整 / 流式都没有才发加载（preview 不算——它就该被完整输出换掉）。
    if (commandOutputFor(cmd) === null && onloadCommand) {
      loadingCmd = id;
      Promise.resolve(onloadCommand(id)).finally(() => {
        loadingCmd = loadingCmd === id ? null : loadingCmd;
      });
    }
  }

  function toggleTool(e: MouseEvent, key: string) {
    e.preventDefault();
    toolOpen = { ...toolOpen, [key]: !toolOpen[key] };
  }

  function toggleThink(e: MouseEvent, key: string) {
    e.preventDefault();
    thinkOpen = { ...thinkOpen, [key]: !thinkOpen[key] };
  }

  function togglePrompt(e: MouseEvent, key: string) {
    e.preventDefault();
    promptOpen = { ...promptOpen, [key]: !promptOpen[key] };
  }

  /** 阶段 prompt 折叠行的读数：两段各报字数，缺的那段不报（步没带 prompt 时空串）。 */
  const promptSummary = (p: { system: string | null; user: string | null } | null) => {
    if (!p) return '';
    const parts: string[] = [];
    if (p.system) parts.push(`系统 ${p.system.length} 字`);
    if (p.user) parts.push(`用户 ${p.user.length} 字`);
    return parts.join(' · ');
  };

  /** 命令是不是还在跑（退出码没落）：在跑的那条流式输出常显，不进折叠。 */
  const isRunning = (exitCode: number | null) => exitCode === null;

  /** 每轮的步骤切片（显尾部：最新的一步与流式尾巴在最底下，窗口化了才看得见「现在」）。 */
  function stepSlice(turn: SceneTurn) {
    return windowSlice(turn.steps, turnPages[turn.key] ?? DEFAULT_PAGE, 'tail');
  }

  /* ── 流式贴底跟随：只在人本就在底上时把视口带下去 ──
     依赖取「在飞轮的正文总量」：它每长一次这条效果就跑一次；人上滑读历史即暂停
     （距底 160px 之外视作在读历史），与对讲台同一姿态、同一套容差量级。 */
  const liveVolume = $derived.by(() => {
    let n = 0;
    for (const t of turns) {
      if (!t.streaming) continue;
      for (const s of t.steps) if (s.streaming || s.kind === 'tool') n += s.text.length;
      n += t.closing.length;
    }
    return n;
  });
  $effect(() => {
    void liveVolume;
    const doc = document.documentElement;
    // 没有可滚的量（含测试环境里几何恒 0）就不跟：jsdom 没有 scrollTo 实现。
    if (doc.scrollHeight <= window.innerHeight) return;
    if (window.innerHeight + window.scrollY < doc.scrollHeight - 160) return;
    void tick().then(() => window.scrollTo({ top: doc.scrollHeight }));
  });

  /* ── 深链 / 跳转落点：把那一轮带到眼前并亮一下边框 ── */
  let flashRun = $state<number | null>(null);
  $effect(() => {
    const rid = highlightRunId;
    if (rid === null) return;
    void tick().then(() => {
      const el = document.querySelector(`article[data-run="${rid}"]`);
      if (!el) return;
      // jsdom 没有 scrollIntoView 实现（测试环境）：亮边框那一段照走。
      if (typeof el.scrollIntoView === 'function') el.scrollIntoView({ block: 'start' });
      flashRun = rid;
      setTimeout(() => {
        if (flashRun === rid) flashRun = null;
      }, 2000);
    });
  });
</script>

{#if filtered.length === 0}
  <!-- 空态的唯一形状（票 13）：状态 → 下一步。查询没命中与「没有记录」分开说。 -->
  <EmptyState
    state={turns.length === 0 ? '这个任务还没有现场记录。' : '没有匹配的轮次。'}
    next={turns.length === 0
      ? '流水线跑起来后，每个节点的会话与命令都会按顺序出现在这里：模型说了什么、跑了哪些命令、输出是什么。'
      : '换个关键词试试——过滤的是名牌、消息正文、命令行与输出。'}
  />
{:else}
  <div class="scenebar">
    <input
      class="qinput"
      type="search"
      placeholder="滤现场：阶段 · 节点 · 正文 · 命令 · 输出"
      aria-label="按阶段、节点、消息正文、命令行或输出过滤现场时间线"
      bind:value={query}
    />
  </div>
  {#if turnSlice.omittedBefore > 0}
    <MoreRow
      label={`已省略前 ${turnSlice.omittedBefore} 轮，点此展开`}
      onclick={() => (shownTurns = Math.min(shownTurns + DEFAULT_PAGE, filtered.length))}
    />
  {/if}

  {#snippet turnBody(t: SceneTurn)}
    {#if !t.loaded && t.steps.length === 0 && !t.closing}
      <div class="quiet">正在读取会话…</div>
    {:else}
      {@const slice = stepSlice(t)}
      {#if slice.omittedBefore > 0}
        <MoreRow
          label={`已省略前 ${slice.omittedBefore} 条，点此展开`}
          onclick={() => (turnPages = { ...turnPages, [t.key]: (turnPages[t.key] ?? DEFAULT_PAGE) + DEFAULT_PAGE })}
        />
      {/if}
      {#each slice.visible as step (step.key)}
        {#if step.kind === 'prompt'}
          <!-- 阶段 prompt（决策 360）：组装后的两段原文快照，默认收起（系统段常以万字计），
               展开体里两段分开摆——「这是 prompt 问题」要能当场核对。 -->
          <details class="rcpt sprompt" open={promptOpen[step.key] ?? false}>
            <summary class="rcpt-head" onclick={(e) => togglePrompt(e, step.key)}>
              <span class="nm">阶段 PROMPT</span>
              <span class="dim args">{promptSummary(step.prompt)}</span>
              <span class="chev" aria-hidden="true">▸</span>
            </summary>
            {#if promptOpen[step.key]}
              <div class="rcpt-more">
                {#if step.prompt?.system}
                  <div class="rm-label dim">系统段</div>
                  <pre class="rm-body mono">{step.prompt.system}</pre>
                {/if}
                {#if step.prompt?.user}
                  <div class="rm-label dim">用户段</div>
                  <pre class="rm-body mono">{step.prompt.user}</pre>
                {/if}
              </div>
            {/if}
          </details>
        {:else if step.kind === 'text'}
          {#if step.role === 'system'}
            <details class="sys">
              <summary class="sys-sum">SYS · 折叠正文 ▸</summary>
              <pre class="sysbox">{step.text}</pre>
            </details>
          {:else if step.role === 'user'}
            <div class="who">YOU</div>
            <pre class="userbox">{step.text}</pre>
          {:else if step.streaming}
            <p class="streaming">{step.text}</p>
          {:else if step.role === 'assistant'}
            <div class="narr"><MarkdownView source={step.text} /></div>
          {:else}
            <pre class="tooltext">{step.text}</pre>
          {/if}
        {:else if step.kind === 'thinking'}
          <!-- 思考步（决策 244 / 359①）：默认收起（它常比回话长一个量级），摘要在流式
               期间带出最新一行原文（ticker，对讲台同款），落地后带字数。 -->
          <details class="rcpt think" open={thinkOpen[step.key] ?? false}>
            <summary class="rcpt-head" onclick={(e) => toggleThink(e, step.key)}>
              <span class="nm">{step.streaming ? '正在想…' : '思考过程'}</span>
              <span class="dim args">{step.streaming ? thinkTicker(step.text) : `${step.text.length} 字`}</span>
              <span class="chev" aria-hidden="true">▸</span>
            </summary>
            <pre class="rm-body mono">{step.text}</pre>
          </details>
        {:else if step.tool}
          {@const tool = step.tool}
          <details
            class="rcpt"
            class:pending={tool.phase === 'running'}
            class:done={tool.phase === 'ok'}
            class:bad={tool.phase === 'bad'}
            open={toolOpen[step.key] ?? false}
          >
            <summary class="rcpt-head" onclick={(e) => toggleTool(e, step.key)}>
              <span class="nm">{tool.name}</span>
              <span class="dim args">{tool.argsSummary}</span>
              <span class="rs" class:bad={tool.phase === 'bad'}>
                {tool.phase === 'running' ? '运行中…' : tool.phase === 'bad' ? '失败' : '完成'}
              </span>
              <span class="chev" aria-hidden="true">▸</span>
            </summary>
            {#if toolOpen[step.key]}
              <div class="rcpt-more">
                {#if tool.args}
                  <div class="rm-label dim">参数</div>
                  <pre class="rm-body mono">{tool.args}</pre>
                {/if}
                <div class="rm-label dim">结果</div>
                <pre class="rm-body mono">{tool.result || '（没有输出）'}</pre>
              </div>
            {/if}
          </details>
        {:else if step.command}
          {@const cmd = step.command}
          {#if isRunning(cmd.exitCode)}
            <!-- 在跑的那条：流式输出常显（正是「用户查看时也流式输出」的那一格） -->
            <div class="rcpt cmd running" data-command={cmd.id}>
              <div class="rcpt-head">
                <i class="lamp"></i>
                <span class="tm">{formatClock(cmd.startedAt)}</span>
                <span class="src">{cmd.source === 'system' ? 'sys' : 'agent'}</span>
                <span class="c mono" title={cmd.command}>{cmd.command}</span>
                <span class="rs">运行中…</span>
              </div>
              {#if cmd.output !== null && cmd.output !== ''}
                <pre class="cmd-live">{cmd.output}</pre>
              {/if}
            </div>
          {:else}
            <details class="rcpt cmd" class:bad={cmd.exitCode !== 0} open={expandedCmd === cmd.id}>
              <summary
                class="rcpt-head"
                onclick={(e) => toggleCmd(e, cmd.id)}
              >
                <i class="lamp" class:bad={cmd.exitCode !== 0}></i>
                <span class="tm">{formatClock(cmd.startedAt)}</span>
                <span class="src">{cmd.source === 'system' ? 'sys' : 'agent'}</span>
                <span class="c mono" title={cmd.command}>{cmd.command}</span>
                {#if cmd.rewritten}<span class="rw">改写</span>{/if}
                <span class="ms">{cmd.durationMs !== null ? formatDuration(cmd.durationMs) : '—'}</span>
                <span class="rs ex" class:bad={cmd.exitCode !== 0}>exit {cmd.exitCode}</span>
                <span class="chev" aria-hidden="true">▸</span>
              </summary>
              {#if expandedCmd === cmd.id}
                {#if loadingCmd === cmd.id && cmd.output === null}
                  <div class="rcpt-more">正在加载完整输出…</div>
                {:else if cmd.outputError}
                  <!-- 读失败就说失败（票 12 / R2-16）：`role=alert` 让读屏也听得到 -->
                  <div class="rcpt-more ferr" role="alert">完整输出没读回来：{cmd.outputError}</div>
                {:else}
                  <div class="rcpt-more">
                    {#if cmd.rewritten}
                      <div class="rm-label dim">→ 实际执行：{cmd.actualCommand}</div>
                    {/if}
                    <pre class="rm-body mono">{cmd.output ?? (cmd.stdoutFile ? '（完整输出未取回，以上是 preview）' : '（该命令未卸载完整输出，只有 preview）')}</pre>
                    <div class="fin">[exit {cmd.exitCode}]{cmd.durationMs !== null ? `  ${formatDuration(cmd.durationMs)}` : ''}</div>
                  </div>
                {/if}
              {/if}
            </details>
          {/if}
        {/if}
      {/each}

      {#if t.closing}
        <!-- 收口话：还在冒时等宽 + 光标（半个 markdown 栅栏会渲染成乱码），落地后 markdown -->
        {#if t.closingStreaming}
          <p class="streaming closing">{t.closing}</p>
        {:else}
          <MarkdownView source={t.closing} class="reply" />
        {/if}
      {/if}
      {#if t.metadata}
        <MetadataCard metadata={t.metadata} />
      {/if}
    {/if}
  {/snippet}

  {#each turnSlice.visible as turn (turn.key)}
    <article
      class="turn"
      class:live={turn.streaming}
      class:flash={flashRun !== null && flashRun === turn.runId}
      data-run={turn.runId ?? undefined}
    >
      <div class="dname">
        {turn.name}{#if turn.sub}
          <span class="sub">∟ {turn.sub}</span>{/if}
        {#if turn.attempt > 1}<span class="att">第 {turn.attempt} 次</span>{/if}
        {#if turn.status && turn.status !== 'success'}<span class="st">{turn.status}</span>{/if}
        {#if turn.tokens}<span class="dim">{formatTokens(turn.tokens.prompt + turn.tokens.completion)} tok</span>{/if}
      </div>

      {#if turn.primary}
        {@render turnBody(turn)}
      {:else}
        <!-- 旧一代的尝试（决策 359③）：整轮折起、内容一个字不删——多次重试的主次
             就在这：最新一代全幅展示，历史按一下就在。 -->
        <details class="retryfold">
          <summary class="retry-sum">
            第 {turn.attempt} 次尝试 · {turn.steps.length} 步 · 点开看全过程
            <span class="chev" aria-hidden="true">▸</span>
          </summary>
          {@render turnBody(turn)}
        </details>
      {/if}
    </article>
  {/each}
{/if}

<style>
  /* 过滤框（旧两页签的 `.rinput` 同一形）：2px 描边、12px 字号是全站像素纪律 */
  .scenebar {
    margin-bottom: 14px;
  }
  .qinput {
    width: 100%;
    max-width: 420px;
    padding: 4px 8px;
    border: 2px solid var(--pane);
    background: var(--panel);
    color: var(--text);
    font-size: 12px;
  }

  /* ── 轮：操作台对话框本体（双线框 + 名牌），取值与 Talk 的 `.turn` / `.dname` 一致 ── */
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
  .turn:first-of-type {
    margin-top: 0;
  }
  .turn :global(.md) {
    max-width: 76ch;
  }
  .dname {
    position: absolute;
    top: -16px;
    left: 6px;
    display: flex;
    align-items: baseline;
    gap: 8px;
    background: var(--bg);
    border: 2px solid var(--pane);
    color: var(--text-hi);
    padding: 0 8px;
    line-height: 1.5;
    white-space: nowrap;
    max-width: calc(100% - 12px);
    overflow: hidden;
  }
  .dname .sub {
    color: var(--text-3);
  }
  .dname .att {
    color: var(--text-3);
    font-family: var(--font-mono);
  }
  .dname .st {
    color: var(--stop);
  }
  .dname .dim {
    color: var(--text-3);
    font-family: var(--font-mono);
  }
  /* 深链 / 跳转落点亮一下：边框走 --go 两秒（不新增动画位） */
  .turn.flash {
    border-color: var(--go);
  }
  .quiet {
    color: var(--text-3);
    font-size: 12px;
  }
  /* ── 旧一代尝试的整轮折叠（决策 359③）：摘要行一抬手就到，内容一个字不删 ── */
  .retryfold {
    margin: 2px 0 0;
  }
  .retry-sum {
    display: flex;
    align-items: center;
    gap: 7px;
    color: var(--text-3);
    cursor: pointer;
    list-style: none;
    font-size: 12px;
  }
  .retry-sum::-webkit-details-marker {
    display: none;
  }
  .retry-sum .chev {
    color: var(--text-4);
  }
  .retryfold[open] > .retry-sum .chev {
    transform: rotate(90deg);
  }
  .who {
    font-family: var(--font-cond);
    font-size: 12px;
    letter-spacing: 0.1em;
    color: var(--text-3);
    margin: 6px 0 3px;
  }
  .userbox {
    color: var(--text-2);
    white-space: pre-wrap;
    font-family: var(--font-ui);
    font-size: 12px;
    line-height: 1.6;
    overflow-wrap: anywhere;
    margin: 0;
  }
  .narr {
    margin: 6px 0;
    color: var(--text);
  }
  .streaming {
    color: var(--text);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-width: 76ch;
    margin: 6px 0;
  }
  .closing {
    border-top: 2px solid var(--hairline);
    padding-top: 6px;
  }
  .tooltext {
    border-left: 4px solid var(--pane);
    background: var(--panel);
    margin: 6px 0 0;
    padding: 6px 10px;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-2);
    white-space: pre-wrap;
    overflow-wrap: break-word;
  }
  .sys {
    margin: 6px 0;
  }
  .sys-sum {
    color: var(--text-3);
    font-size: 12px;
    cursor: pointer;
    list-style: none;
  }
  .sys-sum::-webkit-details-marker {
    display: none;
  }
  .sysbox {
    border: 2px solid var(--pane);
    background: var(--bg);
    padding: 6px 10px;
    color: var(--text-3);
    font-size: 12px;
    font-family: var(--font-mono);
    white-space: pre-wrap;
    max-height: 320px;
    overflow: auto;
  }

  /* ── 回执（工具 / 命令同一形状）：左缘档位说状态，展开体收参数与结果 ── */
  .rcpt {
    border-left: 4px solid var(--pane);
    background: var(--panel);
    padding: 6px 10px;
    margin-top: 6px;
  }
  .rcpt.done {
    border-left-color: var(--go);
  }
  .rcpt.bad {
    border-left-color: var(--stop);
  }
  .rcpt-head {
    display: flex;
    align-items: center;
    gap: 7px;
    color: var(--text-3);
    cursor: pointer;
    list-style: none;
    font-size: 12px;
  }
  .rcpt-head::-webkit-details-marker {
    display: none;
  }
  .rcpt-head .nm {
    color: var(--text-2);
    flex: none;
  }
  .rcpt-head .args {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .rcpt-head .rs {
    margin-left: auto;
    flex: none;
    color: var(--text-3);
  }
  .rcpt-head .rs.bad {
    color: var(--stop);
  }
  .rcpt-head .chev {
    flex: none;
    color: var(--text-4);
  }
  .rcpt[open] > .rcpt-head .chev {
    transform: rotate(90deg);
  }
  .rcpt-more {
    margin-top: 4px;
    font-size: 12px;
  }
  /* 思考步（决策 244）：草稿的视觉——正文比回话淡一档 */
  .rcpt.think .rm-body {
    color: var(--text-3);
  }
  .rm-label {
    font-size: 12px;
    color: var(--text-3);
  }
  .rm-body {
    margin: 2px 0 4px;
    padding: 6px 8px;
    background: var(--pane);
    color: var(--text-2);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: break-word;
    max-height: 320px;
    overflow-y: auto;
    user-select: text;
  }
  .ferr {
    color: var(--stop);
  }
  .fin {
    color: var(--go);
    font-family: var(--font-mono);
    font-size: 12px;
  }

  /* ── 命令回执的摘要行：灯 · 时刻 · 来源 · 命令 · 耗时 · 退出码（旧命令表同列序） ── */
  .rcpt.cmd .c {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-2);
  }
  .rcpt.cmd .tm {
    flex: none;
    color: var(--text-3);
  }
  .rcpt.cmd .src {
    flex: none;
    padding: 0 4px;
    border: 2px solid var(--pane);
    color: var(--text-3);
  }
  .rcpt.cmd .ms {
    flex: none;
    color: var(--text-3);
  }
  .rcpt.cmd .ex {
    margin-left: auto;
  }
  .rw {
    flex: none;
    padding: 0 4px;
    border: 2px solid var(--pane);
    color: var(--text-3);
    font-size: 12px;
  }
  .lamp {
    flex: none;
    width: 8px;
    height: 8px;
    background: var(--go);
  }
  .lamp.bad {
    background: var(--stop);
  }
  .rcpt.cmd.running .lamp {
    background: var(--pending);
  }
  /* 在跑那条的流式输出：等宽、可横滚、限高（它每毫秒都在长） */
  .cmd-live {
    margin: 4px 0 0;
    padding: 6px 8px;
    background: var(--pane);
    color: var(--text-2);
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: break-word;
    max-height: 240px;
    overflow-y: auto;
    user-select: text;
  }

  /* ── 移动版（<480px）：回执头折行、名牌可换行（§5 移动款） ── */
  @media (max-width: 479px) {
    .turn {
      margin-top: 30px;
    }
    .dname {
      white-space: normal;
      flex-wrap: wrap;
    }
    .rcpt-head {
      flex-wrap: wrap;
      gap: 2px 7px;
    }
    .rcpt.cmd .c {
      flex: 1 0 100%;
      order: 9;
      white-space: pre-wrap;
      overflow-wrap: break-word;
    }
    .cmd-live {
      max-height: none;
    }
  }
</style>
