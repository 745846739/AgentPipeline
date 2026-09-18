<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { getGlobalMetrics, getTaskMetrics } from '../api/client';
  import type { GlobalMetrics, TaskMetrics } from '../api/types';
  import EmptyState from '../components/ui/EmptyState.svelte';
  import TrackSegmentBars from '../components/settings/TrackSegmentBars.svelte';
  import { NO_VALIDATE_RUNS_TASK_REASON, mapGlobalMetrics, mapTaskMetrics } from '../lib/metrics';
  import { CompositionGuard, shouldSubmitOnEnter } from '../lib/enterToSend';
  import { router } from '../router.svelte';

  /**
   * 全局指标（design/frontend-design.md §7 / theme-6-pixel.md §3.1「车间台账」）：
   * 成功率 / 各阶段平均耗时 / 重试率 / 逃逸事件 / 首过率，全部以「轨道分段条形图」
   * 呈现（横条挂在轨道站点下），**无 KPI 卡片横排**——「轨道即导航」在指标页成立。
   *
   * **第一段说人话**（UX 审计票 27 / 用户故事 22）：四个量各自是什么、怎么算的、该往哪个
   * 方向看；原始字段名（`total_tokens` / `calls`）与内部决策编号都不进正文（编号退场见票 23，
   * 兜底机器门在 `lib/copy-discipline.test.ts`）。分母为 0 的比值**不画一条没有解释的横线**：
   * 整条换成一句「为什么暂时没有意义」（成功率与首过率同一处置，原因文案在 `lib/metrics.ts`）。
   * 数据源与字段映射逐字不变（`lib/metrics.ts`，决策 130 / 137）。
   *
   * **任务级入口**（UX 审计票 06）：`#/metrics?task=<task_id>` 据 `route.query.task`
   * **自动载入并高亮**该任务的指标，不再要求用户手打 ULID（输入框保留为兜底）。
   *
   * **空态**（票 13）：一个任务都没有时用 `EmptyState`——状态 → 下一步 → 可点的入口。
   */

  let global = $state<GlobalMetrics | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let taskIdInput = $state('');
  let taskMetrics = $state<TaskMetrics | null>(null);
  /** 当前展示的是哪个任务的指标（票 06：据链接就位的那一份要认得出来）。 */
  let loadedTaskId = $state<string | null>(null);
  /** 已经把链接里的哪个任务就位过了（与 `loadedTaskId` 分开记：手输载入另一个任务时，
   *  不该因为「当前任务」变了又把链接那个任务拉回来）。 */
  let appliedQueryTask = $state<string | null>(null);
  let taskLoading = $state(false);
  let taskError = $state<string | null>(null);
  /** 输入法组合态（决策 184）：输入法里敲字再回车是选字，不该直接去查指标。 */
  const composing = new CompositionGuard();

  const view = $derived(global ? mapGlobalMetrics(global) : null);
  const taskView = $derived(taskMetrics ? mapTaskMetrics(taskMetrics) : null);
  /** 求和口径与任务表持久化口径对不上（观测提示，不是错误）。 */
  const taskDrift = $derived(taskView ? taskView.tokenDrift || taskView.callsDrift : false);

  /**
   * 任务指标入口的 query（票 06）：`#/metrics?task=<task_id>`。形状是冻结契约
   * （`parallel-brief.md` §二 的跨流接口表），参数名**逐字照抄**；由 N 的 `router.svelte.ts`
   * 解析成已解码的 `query`（没有查询串时是空对象）。读不到时本页行为不变（照旧等人手输）。
   */
  const queryTaskId = $derived(router.route.query.task?.trim() ?? '');

  async function loadGlobal() {
    loading = true;
    error = null;
    try {
      global = await getGlobalMetrics();
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  }

  onMount(() => void loadGlobal());

  async function loadTask(id: string) {
    const trimmed = id.trim();
    if (!trimmed) {
      taskError = '请填写任务 ID。';
      return;
    }
    taskLoading = true;
    taskError = null;
    try {
      taskMetrics = await getTaskMetrics(trimmed);
      loadedTaskId = trimmed;
    } catch (err) {
      taskMetrics = null;
      loadedTaskId = null;
      taskError = (err as Error).message;
    } finally {
      taskLoading = false;
    }
  }

  /**
   * 带任务 ID 打开时**自动载入并高亮**（票 06）——不再要求用户手打一个记不住的 ULID。
   * 链接由任务详情侧放（跨流接口）；输入框只是兜底，不是唯一的路。
   * 载入完成后把那一份拉到眼前（上面还有全局那几张图，不拉会看不见自己点的那份）。
   *
   * 只认「链接里那个 id 还没就位过」这一条：手输换成别的任务时不把链接那个拉回来。
   */
  $effect(() => {
    const id = queryTaskId;
    if (!id || id === appliedQueryTask) return;
    appliedQueryTask = id;
    taskIdInput = id;
    void (async () => {
      await loadTask(id);
      await tick();
      document.querySelector('.task.current')?.scrollIntoView({ block: 'nearest' });
    })();
  });
</script>

<div class="page">
  <a class="crumb" href="#/">看板</a>
  <div class="p-head">
    <h1 class="p-title">全局指标</h1>
    <button type="button" class="btn" disabled={loading} onclick={() => loadGlobal()}>
      {#if loading}<span class="spin"></span>{/if}刷新
    </button>
  </div>

  {#if error}
    <div class="blank error">{error}</div>
  {:else if loading && !global}
    <div class="blank">正在加载指标…</div>
  {:else if global && view}
    <!-- 第一段：平实说法讲清四个量是什么、怎么算、往哪个方向看（票 27）。
         这一档灰是「次级必读」（决策 195 / brief §三.1），不是装饰档。 -->
    <p class="hintline">
      下面这几个数说的是整条流水线现在的样子，都从现有任务里算出来。
      <b>成功率</b>：跑完的任务里有多少是成功的（失败、取消也算跑完，还在跑的不算）。
      <b>重试率</b>：每个阶段被重做过的比例（同一阶段第一次没过、又跑一遍就记一次）。
      <b>逃逸事件</b>：活被打回重做的次数（上游没拦住、后面才发现问题），按发起打回的那个阶段归类。
      <b>首过率</b>：质量检查第一次就通过的比例（第一次没过、改完再查才算通过的，不算首过）。
      成功率与首过率越高越好；重试率与逃逸事件越低越好。
      {#if global.tasks > 0}
        现在一共 <b>{global.tasks}</b> 个任务，逃逸事件 <b>{view.escapeEvents}</b> 次，
        到现在用掉 <b>{view.tokenDisplay}</b> 个 token、调用了 <b>{view.callsDisplay}</b> 次模型。
      {/if}
    </p>

    {#if global.tasks === 0}
      <!-- 空态（票 13）：状态 → 下一步 → 可点的入口，七个页面同一套语汇。 -->
      <EmptyState
        state="现在还没有任务，所以没有可统计的东西。"
        next="先去看板新建一个任务：它跑起来之后，这里会出现成功率、各阶段耗时与重试率。"
        href="#/"
        linkLabel="去看板新建任务"
      />
    {:else}
      <!-- 站点分段条形图：横条挂在传送带站点下，无 KPI 卡片横排 -->
      <TrackSegmentBars
        title="成功率"
        subtitle="跑完的任务里成功的比例"
        bars={view.success}
        emptyText={view.successReason}
        ariaLabel="任务成功率"
      />

      <TrackSegmentBars
        title="各阶段平均耗时"
        subtitle="每个阶段平均花多久；横条按最长的那个阶段拉满"
        bars={view.duration}
        emptyText="还没有节点运行记录。"
        ariaLabel="各阶段平均耗时"
      />

      <TrackSegmentBars
        title="各阶段重试率"
        subtitle="同一阶段重做过一遍的比例"
        bars={view.retry}
        emptyText="还没有节点运行记录。"
        ariaLabel="各阶段重试率"
      />

      <TrackSegmentBars
        title="逃逸事件（按打回的来源阶段）"
        subtitle="打回重做的次数；条长按最多的那个阶段拉满"
        bars={view.escape}
        emptyText="还没有打回记录。"
        ariaLabel="逃逸事件分布"
      />

      <TrackSegmentBars
        title="首过率"
        subtitle="质量检查第一次就通过的比例"
        bars={view.firstPass}
        emptyText={view.firstPassReason}
        ariaLabel="首过率"
      />

      {#if view.excludedStages.length > 0}
        <p class="excl">
          这几张图挂在传送带（这条流水线的顺序）上，只画真正占阶段的站；
          {view.excludedStages.join('、')} 不是阶段，没有画进来。
        </p>
      {/if}
    {/if}
  {/if}

  <section class="chart panel task" class:current={loadedTaskId !== null}>
    <div class="chart-head">
      <h2>任务级指标</h2>
      {#if loadedTaskId}<span class="sub mono">任务 {loadedTaskId}</span>{/if}
    </div>
    <p class="lead">
      从任务详情页的指标入口过来时，这里会自动载入那个任务；也可以在下面直接填它的任务 ID。
    </p>
    <div class="subform">
      <input
        class="input mono"
        bind:value={taskIdInput}
        placeholder="任务 ID（ULID）"
        onkeydown={(e) => {
          if (!shouldSubmitOnEnter(e, composing.active())) return;
          e.preventDefault();
          void loadTask(taskIdInput);
        }}
        oncompositionstart={() => composing.start()}
        oncompositionend={() => composing.end()}
      />
      <button type="button" class="btn" disabled={taskLoading} onclick={() => loadTask(taskIdInput)}>
        {#if taskLoading}<span class="spin"></span>{/if}载入
      </button>
    </div>

    {#if taskError}
      <div class="blank error" role="alert">{taskError}</div>
    {:else if taskView && taskMetrics}
      <div class="msum mono">
        <span>
          这个任务用了 <b>{taskView.tokenDisplay}</b> 个 token、调用了
          <b>{taskView.callsDisplay}</b> 次模型
        </span>
        <span class={taskDrift ? 'drift' : ''}>
          任务表里记的是 {taskView.storedTokenDisplay} 个 token、{taskView.storedCallsDisplay} 次
        </span>
      </div>
      {#if taskDrift}
        <p class="drift-note">
          任务表里记的数与按执行记录逐个加起来的结果有出入（父子行的口径不同，或者还没落库）。
        </p>
      {/if}

      <div class="task-charts">
        <TrackSegmentBars
          title="阶段平均耗时"
          bars={taskView.duration}
          emptyText="该任务还没有节点运行记录。"
          ariaLabel="任务阶段平均耗时"
        />
        <TrackSegmentBars
          title="阶段重试率"
          bars={taskView.retry}
          emptyText="该任务还没有节点运行记录。"
          ariaLabel="任务阶段重试率"
        />
        <!-- 首过率缺数据：整条换成一句「为什么没有意义」，不画 0 也不画一条光秃秃的横线（票 27） -->
        <TrackSegmentBars
          title="首过率"
          bars={taskView.firstPass}
          emptyText={NO_VALIDATE_RUNS_TASK_REASON}
          ariaLabel="任务首过率"
        />
      </div>
    {/if}
  </section>
</div>

<style>
  .page {
    max-width: var(--detail-max);
    margin: 0 auto;
    padding: 18px 20px 44px;
  }
  /* 页头：面包屑 + 24px 标题 + 右侧动作钮（§3.1） */
  .crumb {
    display: inline-block;
    color: var(--text-3);
    margin-bottom: 10px;
  }
  .crumb::before {
    content: '← ';
  }
  .crumb:hover {
    color: var(--text-hi);
  }
  .p-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 14px;
    margin: 6px 0 8px;
  }
  .p-title {
    font-size: 24px;
    font-weight: 400;
    color: var(--text-hi);
    line-height: 1.2;
  }
  /* 导入语：**次级必读**档（决策 195 / brief §三.1）——读不到就不知道这些数在说什么 */
  .hintline {
    color: var(--text-3);
    line-height: 1.8;
    margin-bottom: 14px;
    max-width: 86ch;
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
  .excl {
    color: var(--text-3);
    margin-top: 8px;
    line-height: 1.8;
    max-width: 86ch;
  }
  /* 任务级台账盒：同一套 2px 描边 + 硬投影，只是内容更密 */
  .task {
    margin-top: 24px;
    padding: 12px 14px;
  }
  /* 票 06：据 `?task=<id>` 就位的那一份。这一页的「当前任务」就是链接指过来的那个任务，
     故不是选中态——只是「你要看的就是这一份」的位置标记（左缘亮描边 + wash 底），
     与设置·项目据 query 标出的那一行同一处置。
     描边用像素纪律里那条唯一的例外写法 `border-left: 4px`（其余描边一律 2px）；
     `.panel` 本来有 2px 左边框，故 `padding-left` 只抵掉多出来的 2px（14 − 2 = 12），
     内容不错位；硬投影沿用 `.panel` 的 `4px 4px 0`，不再重述。 */
  .task.current {
    background: var(--wash);
    border-left: 4px solid var(--text-hi);
    padding-left: 12px;
  }
  .chart-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin-bottom: 12px;
  }
  .chart-head h2 {
    font-size: 12px;
    font-weight: 400;
    letter-spacing: 0.08em;
    color: var(--text-hi);
  }
  .sub {
    font-size: 12px;
    color: var(--text-3);
  }
  /* 兜底入口的说明（票 06）：手输不是唯一的路，所以得说清正路在哪 */
  .lead {
    color: var(--text-3);
    line-height: 1.8;
    margin-bottom: 10px;
    max-width: 86ch;
  }
  .subform {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }
  .subform .input {
    max-width: 360px;
  }
  .msum {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
    margin-top: 12px;
    color: var(--text-2);
  }
  /* 口径差异是**读数提示**，不是有东西要你处理：提亮到可读档即可，不用告警琥珀
     （票 12 / brief §三.5；信号色作文本色也与决策 195 §3 的用法规约冲突）。 */
  .msum .drift {
    color: var(--text-hi);
  }
  .drift-note {
    margin-top: 5px;
    color: var(--text-3);
  }
  .task-charts {
    margin-top: 12px;
  }
  .task-charts :global(.chart:last-child) {
    margin-bottom: 0;
  }
</style>
