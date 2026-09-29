<script lang="ts">
  import type { NodeCommand } from '../../api/types';
  import { filterCommands, type ExitFilter } from '../../lib/commandFilter';
  import { formatClock } from '../../lib/format';
  import { formatDuration } from '../../lib/pipeline';
  import { DEFAULT_PAGE, nextPage, windowSlice } from '../../lib/windowSlice';
  import EmptyState from '../ui/EmptyState.svelte';
  import MoreRow from '../ui/MoreRow.svelte';

  interface Props {
    commands: NodeCommand[];
    /** 完整输出（已卸载文件 / 已加载）。 */
    outputFor?: (command: NodeCommand) => string | null;
    /** 请求加载完整输出 `GET /commands/{id}/output`。 */
    onload?: (commandId: number) => Promise<void> | void;
    /** SSE 追加的输出（进行中的命令）。 */
    streamedFor?: (command: NodeCommand) => string | null;
    /**
     * 完整输出的读取错误（票 12 / R2-16）。此前这个错误**存了没人读**：
     * `stores/taskDetail` 把它记在 `commandOutputError` 里，而这里在取不到时永远显示
     * 「（正在加载完整输出…）」——一句能永久停住的谎。
     */
    errorFor?: (command: NodeCommand) => string | null;
  }

  let { commands, outputFor, onload, streamedFor, errorFor }: Props = $props();

  let expanded = $state<number | null>(null);
  let loading = $state<number | null>(null);

  /**
   * 长列表窗口化（spec list-windowing 票 02）：数百条命令全量平铺不可读——过滤与切片
   * 叠加，判据是**先过滤后切**（过滤后的名单喂 `windowSlice`，默认显尾部 50 条：
   * 最新的一条永远在场）。过滤状态不进 URL，与折叠态同一口径（决策 217 类比）。
   */
  let keyword = $state('');
  let exitFilter = $state<ExitFilter>('all');
  let shown = $state(DEFAULT_PAGE);

  const filtered = $derived(filterCommands(commands, keyword, exitFilter));
  const slice = $derived(windowSlice(filtered, shown, 'tail'));

  // 换过滤档（关键词 / 退出码）时窗口游标回缺省：省略计数是按当前名单算的，旧游标只会有害
  $effect(() => {
    keyword;
    exitFilter;
    shown = DEFAULT_PAGE;
  });

  async function toggle(command: NodeCommand) {
    if (expanded === command.id) {
      expanded = null;
      return;
    }
    expanded = command.id;
    const has = outputFor?.(command) ?? streamedFor?.(command);
    if (!has && onload) {
      loading = command.id;
      try {
        await onload(command.id);
      } finally {
        loading = command.id === loading ? null : loading;
      }
    }
  }

  /**
   * 折叠行上显示哪一条（决策 297）：**有原串就显示原串**——那才是模型（或项目配置）想要
   * 的那件事，排障的人先要看到的是「它想干什么」而不是「这条命令被换成了什么」。
   * `original_command` 为 `null` 的行（没改写过，绝大多数）显示的就是实际执行的那条。
   */
  function shownCommand(command: NodeCommand): string {
    return command.original_command ?? command.command;
  }

  /** 这一行真的被改写了吗（展开时才需要把两条都摆出来）。 */
  function wasRewritten(command: NodeCommand): boolean {
    return command.original_command !== null && command.original_command !== command.command;
  }

  function outputText(command: NodeCommand): string {
    return (
      outputFor?.(command) ??
      streamedFor?.(command) ??
      command.stdout_preview ??
      (command.stdout_path ? '（完整输出未取回，以上是 preview）' : '（该命令未卸载完整输出，只有 preview）')
    );
  }
</script>

{#if commands.length === 0}
  <!-- 空态的唯一形状（票 13）：状态 → 下一步。 -->
  <EmptyState
    state="还没有命令记录。"
    next="节点每跑一条命令都会记在这里：命令、耗时、退出码，点开看完整输出。"
  />
{:else}
  <div class="filters">
    <input
      class="finput"
      type="search"
      placeholder="搜命令行…"
      aria-label="按命令行关键词过滤"
      bind:value={keyword}
    />
    <div class="fexits" role="group" aria-label="按退出码过滤">
      {#each [['all', '全部'], ['nonzero', '非零'], ['zero', '零']] as [key, label] (key)}
        <button
          type="button"
          class="fopt"
          class:on={exitFilter === key}
          aria-pressed={exitFilter === key}
          onclick={() => (exitFilter = key as ExitFilter)}
        >{label}</button>
      {/each}
    </div>
  </div>
  {#if slice.omittedBefore > 0}
    <MoreRow
      label={`已省略前 ${slice.omittedBefore} 条，点此展开`}
      onclick={() => (shown = nextPage(shown, filtered.length))}
    />
  {/if}
  <div class="cmds">
    {#each slice.visible as command (command.id)}
      <button type="button" class="cmd" onclick={() => toggle(command)}>
        <span class="cmd-l1">
          <i class={command.exit_code === null || command.exit_code === 0 ? 'ok' : 'bad'}></i>
          <span class="tm">{formatClock(command.started_at)}</span>
          <span class="src">{command.source === 'system' ? 'sys' : 'agent'}</span>
          <span class="ms">{command.duration_ms !== null ? formatDuration(command.duration_ms) : '—'}</span>
          <span class="ex {command.exit_code && command.exit_code !== 0 ? 'bad' : ''}">
            exit {command.exit_code ?? '—'}
          </span>
        </span>
        <span class="cmd-l2">
          <span class="c" title={shownCommand(command)}>{shownCommand(command)}</span>
          {#if wasRewritten(command)}
            <span class="rw" title="这一条被 rtk 改写后再执行">改写</span>
          {/if}
        </span>
      </button>
      {#if expanded === command.id}
        {#if loading === command.id}
          <div class="cmdout">正在加载完整输出…</div>
        {:else if errorFor?.(command)}
          <!-- 读失败就说失败（票 12 / R2-16）：`role=alert` 让读屏也听得到 -->
          <div class="cmdout failed" role="alert">
            完整输出没读回来：{errorFor?.(command)}
          </div>
        {:else}
          <pre class="cmdout">{#if wasRewritten(command)}<span class="ln">$ {command.original_command}</span>
<span class="rw">→ 实际执行：{command.command}</span>
{:else}<span class="ln">$ {command.command}</span>
{/if}{outputText(command)}{#if command.exit_code !== null}
<span class="fin">[exit {command.exit_code}]{command.duration_ms !== null ? `  ${formatDuration(command.duration_ms)}` : ''}</span>{/if}</pre>
        {/if}
      {/if}
    {/each}
  </div>
{/if}

<style>
  .cmds {
    max-width: 900px;
  }
  /* 过滤行（票 02）：输入框与退出码三档。描边 2px、字号 12px 是全站像素纪律 */
  .filters {
    display: flex;
    gap: 6px;
    align-items: center;
    max-width: 900px;
    margin-bottom: 8px;
  }
  .finput {
    flex: 1;
    min-width: 0;
    padding: 4px 8px;
    border: 2px solid var(--pane);
    background: var(--panel);
    color: var(--text);
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .fexits {
    display: flex;
    flex: none;
  }
  .fopt {
    padding: 4px 8px;
    border: 2px solid var(--pane);
    background: none;
    color: var(--text-3);
    font-size: 12px;
    cursor: pointer;
  }
  .fopt + .fopt {
    border-left: 0;
  }
  .fopt.on {
    background: var(--wash);
    color: var(--text-hi);
  }
  /* 「改写」标：只在真的换过命令的行上出现（决策 297）——不喧哗，但一眼看得出这条
     跑的不是它写的那个样子。描边 2px、字号 12px 是 §3.1 / §5 的全站像素纪律：1px 与
     11px 会被 `theme/css-parity.test.ts` 当场拦下。 */
  .rw {
    margin-left: 6px;
    padding: 0 4px;
    border: 2px solid var(--pane);
    color: var(--text-3);
    font-size: 12px;
  }
  .cmdout .rw {
    display: block;
    margin: 0 0 4px;
    padding: 0;
    border: 0;
    color: var(--text-3);
  }
  /* 桌面单表多列：命令 · 时间 · 来源徽标 · 耗时 · 退出码；移动款两行制见下方 */
  .cmd {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 8px;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-2);
    cursor: pointer;
    width: 100%;
    text-align: left;
  }
  .cmd:hover {
    background: var(--hover-bg);
  }
  /* 退出码信号灯：8px 实心像素方块（绿灯 = 成功，红灯 = 非零），不用字符 ✓/✗ */
  .ok,
  .bad {
    flex: none;
    width: 8px;
    height: 8px;
  }
  .ok {
    background: var(--go);
  }
  .bad {
    background: var(--stop);
  }
  .tm {
    color: var(--text-3);
    width: 62px;
    flex: none;
  }
  /* source = agent | system 仍用徽标区分，但共用一表 */
  .src {
    flex: none;
    width: 42px;
    font-size: 12px;
    text-align: center;
    border: 2px solid var(--pane);
    color: var(--text-3);
  }
  .c {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ms {
    color: var(--text-3);
    width: 52px;
    flex: none;
    text-align: right;
  }
  .ex {
    color: var(--text-3);
    width: 64px;
    flex: none;
    text-align: right;
  }
  .ex.bad {
    color: var(--stop);
  }
  /* 输出块：2px 左缘像素条 + 货箱面底，等宽不折行可横滚 */
  .cmdout {
    border-left: 2px solid var(--pane);
    background: var(--panel);
    margin: 4px 0 12px 26px;
    padding: 8px 12px;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-2);
    max-width: 840px;
    white-space: pre-wrap;
    overflow: auto;
    max-height: 360px;
  }
  .cmdout .ln {
    color: var(--text-3);
  }
  .cmdout .fin {
    color: var(--go);
  }
  .cmdout.failed {
    border-left-color: var(--stop);
    color: var(--stop);
  }

  /* 桌面：两行容器不生成盒子，用 order 还原原列序（ok · tm · src · 命令 · ms · ex · 输出） */
  .cmd-l1,
  .cmd-l2 {
    display: contents;
  }
  .cmd .c {
    order: 1;
  }
  .cmd .ms {
    order: 2;
  }
  .cmd .ex {
    order: 3;
  }

  /* ── 移动版（<480px）：命令表两行制（§5 移动款） ── */
  @media (max-width: 479px) {
    .cmds {
      max-width: none;
    }
    .cmd {
      flex-wrap: wrap;
      align-items: center;
      gap: 2px 9px;
      padding: 9px 2px;
      border-bottom: 2px solid var(--hairline);
    }
    .cmd:last-child {
      border-bottom: 0;
    }
    .cmd .cmd-l1 {
      display: flex;
      flex: 1 0 100%;
    }
    .cmd .cmd-l1 .ms,
    .cmd .cmd-l1 .ex {
      order: 0;
    }
    .cmd .cmd-l1 .ms {
      width: auto;
      margin-left: auto;
    }
    .cmd .cmd-l1 .ex {
      width: auto;
    }
    .cmd .cmd-l2 {
      display: flex;
      flex: 1 0 100%;
      margin-top: 3px;
    }
    .cmd .cmd-l2 .c {
      overflow: visible;
      text-overflow: clip;
      white-space: pre-wrap;
      word-break: break-all;
      font-size: 12px;
      color: var(--text);
      line-height: 1.5;
    }
    .cmdout {
      margin: 8px 0 12px 0;
      max-height: none;
      font-size: 12px;
    }
  }
</style>
