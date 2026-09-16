<script lang="ts">
  import { board, FILTER_LABELS, type StatusFilter } from '../../stores/board.svelte';
  import { router } from '../../router.svelte';
  import NewTaskDialog from '../board/NewTaskDialog.svelte';
  import Sprite from '../render/Sprite.svelte';
  import { BOARD_COLUMNS, columnForTask } from '../../lib/pipeline';
  import type { SpriteName } from '../../theme/contract';

  const FILTERS: StatusFilter[] = ['all', 'running', 'pending', 'waiting', 'queued', 'done', 'ended'];

  /**
   * 过滤项 → 受控 sprite（决策 169 / theme-6-pixel.md §3「道具栏槽位」）。
   * 图元只来自契约的 sprite 表；`all`=货箱、`pending`=急停三角、`ended`=折返锤。
   * 七个过滤项全保留（原型只画六格；删过滤项会改交互契约，见票面说明）。
   */
  const SLOT_SPRITES: Record<StatusFilter, SpriteName> = {
    all: 'chest',
    running: 'gear',
    pending: 'alert',
    waiting: 'merge',
    queued: 'flag',
    done: 'trophy',
    ended: 'hammer',
  };

  const NAV: Array<{ path: string; route: string; label: string; sprite: SpriteName }> = [
    // 对讲台（决策 174 / theme-6-pixel.md §3.3）：与看板并列，故排在台账页之前。
    // 图元复用既有 foreman 头像（不新增 sprite）；它在 34px 页签盒里按 16px 显示。
    { path: '/talk', route: 'talk', label: '对讲台', sprite: 'foreman' },
    { path: '/metrics', route: 'metrics', label: '指标', sprite: 'chart' },
    { path: '/settings/projects', route: 'settings-projects', label: '项目', sprite: 'chest' },
    { path: '/settings/providers', route: 'settings-providers', label: '模型与密钥', sprite: 'key' },
    // 技能市场（决策 187）：与「模型与密钥」并列的设置页；sprite 复用既有的 merge（来源接入）。
    { path: '/settings/market', route: 'settings-market', label: '技能市场', sprite: 'merge' },
    { path: '/share', route: 'share', label: '手机访问', sprite: 'phone' },
  ];

  let newTaskOpen = $state(false);

  const sessionName = $derived(
    board.projects.find((p) => p.id === board.projectId)?.name ?? 'AgentPipeline',
  );

  /**
   * 移动版信号灯缩略条（theme-6-pixel.md §5 / 移动原型 `.railnav`）：
   * 8 列各压成一枚 10px 实心像素灯，链节底纹连通。字符字形（○ ● ◆）已退役。
   */
  const railCells = $derived.by(() =>
    BOARD_COLUMNS.map((column) => {
      const tasks = board.tasks.filter((t) => columnForTask(t) === column.key);
      const pen = tasks.some((t) => t.status === 'pending');
      const live = tasks.some((t) => t.status === 'running');
      const don = tasks.length > 0 && tasks.every((t) => t.status === 'done');
      const state = pen ? 'pen' : live ? 'live' : don ? 'don' : 'idle';
      return { key: column.key, label: column.label, state };
    }),
  );

  function openTask(id: string) {
    board.pendingOpen = false;
    router.navigate(`/task/${id}`);
  }

  /**
   * 点信号灯缩略条跳段（移动原型 `.rn` + `scrollIntoView`）。
   * 站点带带 `scroll-margin-top: 148px`，故跳到顶时不会被 138px 的顶栏压住。
   */
  function jumpToStation(key: string) {
    if (typeof document === 'undefined') return;
    document.getElementById(`s-${key}`)?.scrollIntoView({ block: 'start' });
  }
</script>

<header class="top">
  <div class="topbar">
    <div class="bar-top">
      <span class="logo" aria-hidden="true"></span>
      <a class="wordmark" href="#/" onclick={() => router.navigate('/')}>AGENTPIPELINE</a>
      <span class="sess">0:{sessionName}</span>

      <nav class="railnav" aria-label="站点状态缩略">
        {#each railCells as cell (cell.key)}
          <!-- 点灯跳段（移动原型 `.rn` → `scrollIntoView`）；桌面隐藏 -->
          <button
            type="button"
            class="rn {cell.state}"
            aria-label="跳到 {cell.label}"
            onclick={() => jumpToStation(cell.key)}
          >
            <span class="mk"></span>
          </button>
        {/each}
      </nav>
    </div>

    <div class="filters-row">
      <nav class="slots" aria-label="状态过滤">
        {#each FILTERS as f (f)}
          <button
            type="button"
            class="slot {board.filter === f ? 'on' : ''} {f === 'pending' ? 'pend' : ''}"
            title={FILTER_LABELS[f]}
            aria-label="{FILTER_LABELS[f]}（{board.countFor(f)}）"
            aria-pressed={board.filter === f}
            onclick={() => board.setFilter(f)}
          >
            <Sprite name={SLOT_SPRITES[f]} />
            <span class="cb">{board.countFor(f)}</span>
          </button>
        {/each}
      </nav>

      <div class="pending-wrap">
        <button
          type="button"
          class="chip pending-count {board.pendingCount > 0 ? 'pend' : ''}"
          aria-expanded={board.pendingOpen}
          onclick={() => board.togglePendingDropdown()}
        >
          待处理 <span class="c">{board.pendingCount}</span>
        </button>
        {#if board.pendingOpen}
          <div class="dropdown panel" role="menu">
            {#if board.pendingTasks.length === 0}
              <div class="dd-empty">当前没有待办任务。</div>
            {:else}
              {#each board.pendingTasks as task (task.id)}
                <button
                  type="button"
                  class="dd-item"
                  role="menuitem"
                  onclick={() => openTask(task.id)}
                >
                  <span class="dd-dot"></span>
                  <span class="dd-title">{task.title}</span>
                  <span class="dd-msg">{task.pending_reason?.message ?? ''}</span>
                </button>
              {/each}
            {/if}
          </div>
        {/if}
      </div>

      <button type="button" class="btn btn-new" onclick={() => (newTaskOpen = true)}>新建任务</button>
    </div>
  </div>

  <nav class="navbar" aria-label="页面导航">
    {#each NAV as item (item.path)}
      <a
        href="#{item.path}"
        class="chip navchip"
        class:on={router.route.name === item.route}
        onclick={() => router.navigate(item.path)}
      >
        <span class="ic"><Sprite name={item.sprite} /></span>{item.label}
      </a>
    {/each}

    {#if board.projects.length > 0}
      <label class="proj">
        <span class="visually-hidden">项目</span>
        <select
          value={board.projectId ?? ''}
          onchange={(e) => board.selectProject((e.currentTarget as HTMLSelectElement).value)}
        >
          {#each board.projects as p (p.id)}
            <option value={p.id}>{p.name}</option>
          {/each}
        </select>
      </label>
    {/if}
  </nav>
</header>

<NewTaskDialog open={newTaskOpen} onclose={() => (newTaskOpen = false)} />

<style>
  /* 顶栏 = 车间铭牌（sticky）：第一行铭牌 + 道具栏，第二行页面铭牌排（§3） */
  .top {
    position: sticky;
    top: 0;
    z-index: 20;
    background: var(--bg);
    border-bottom: 2px solid var(--pane);
  }
  .topbar {
    display: flex;
    align-items: center;
    gap: 14px;
    height: 46px;
    padding: 0 16px;
  }
  /* 移动版专属分组容器：桌面拍平（children 直接成为 .topbar 的 flex 子项） */
  .bar-top,
  .filters-row {
    display: contents;
  }
  /* 铭牌灯：实心绿方块 + 2px 墨描边（原型 7px/5px 琥珀投影与像素纪律冲突，已去） */
  .logo {
    flex: none;
    width: 12px;
    height: 12px;
    background: var(--go);
    border: 2px solid var(--ink);
  }
  /* wordmark：全站唯一带文字投影的元素（浅色款去投影，§2.4 偏差①） */
  .wordmark {
    flex: none;
    font-size: 24px;
    color: var(--text-hi);
    letter-spacing: 0.08em;
    line-height: 1;
    text-shadow: 3px 3px 0 var(--ink);
    white-space: nowrap;
  }
  :global(html[data-theme='light']) .wordmark {
    text-shadow: none;
  }
  .wordmark:hover {
    text-decoration: none;
  }
  .sess {
    color: var(--text-3);
    font-size: 12px;
    white-space: nowrap;
  }
  .railnav {
    display: none;
  }

  /* ── 道具栏槽位：34px、2px 描边、右下角计数徽章、选中 = 亮描边 + wash 底 ── */
  .slots {
    display: flex;
    align-items: center;
    margin-left: auto;
    /* 上下留出计数徽章的 -5px 溢出，否则被 overflow 裁掉 */
    padding: 5px 6px 5px 0;
    min-width: 0;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .slots::-webkit-scrollbar {
    display: none;
  }
  .slot {
    position: relative;
    flex: none;
    width: 34px;
    height: 34px;
    margin-left: -2px;
    border: 2px solid var(--pane);
    background: var(--panel);
    display: grid;
    place-items: center;
    color: var(--text-2);
  }
  .slot:first-child {
    margin-left: 0;
  }
  .slot:hover {
    color: var(--text-hi);
  }
  .slot.on {
    border-color: var(--text-hi);
    color: var(--text-hi);
    background: var(--wash);
    z-index: 1;
  }
  .slot .cb {
    position: absolute;
    right: -5px;
    bottom: -5px;
    background: var(--bg);
    border: 2px solid var(--pane);
    color: var(--text-3);
    padding: 0 2px;
    line-height: 1.2;
    font-variant-numeric: tabular-nums;
  }
  .slot.pend {
    color: var(--pending);
  }
  .slot.pend .cb {
    color: var(--pending);
    border-color: var(--pending);
  }

  /* ── 页面铭牌排：像素芯片（选中态与槽位一致：亮描边 + wash 底） ── */
  .navbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 16px 6px;
    overflow-x: auto;
    scrollbar-width: none;
  }
  .navbar::-webkit-scrollbar {
    display: none;
  }
  .chip {
    flex: none;
    display: inline-flex;
    align-items: center;
    border: 2px solid transparent;
    color: var(--text-2);
    padding: 1px 8px;
    line-height: 1.5;
    white-space: nowrap;
  }
  .chip:hover {
    color: var(--text-hi);
    text-decoration: none;
  }
  .chip.on {
    background: var(--wash);
    border-color: var(--text-hi);
    color: var(--text-hi);
  }
  .chip.on::before {
    content: '▶ ';
    color: var(--go);
  }
  .chip .ic {
    display: inline-flex;
    margin-right: 5px;
    vertical-align: -3px;
  }
  /* 工头头像的契约显示尺寸是 48px（dossier 用），塞进 34px 高的导航页签盒会把它
     撑到 52px，连带移动端顶栏从 138px 涨到 156px，压坏 §5 的 scroll-margin-top
     与横幅 top = 148px。导航处一律按 chip 节奏缩到 16px（规格 §3.3）。 */
  .chip .ic :global(svg.sprite) {
    width: 16px;
    height: 16px;
  }
  .chip .c {
    display: inline-block;
    min-width: 14px;
    text-align: center;
    color: var(--text-3);
    border: 2px solid var(--pane);
    padding: 0 2px;
    margin-left: 4px;
    line-height: 1.3;
    font-variant-numeric: tabular-nums;
  }
  .chip.pend .c {
    color: var(--pending);
    border-color: var(--pending);
  }

  /* 项目选择器必须自己当定位包含块：它内部的无障碍标签是 `position: absolute`
     （`.visually-hidden`），而 `.proj` 未定位时那块绝对定位的盒子会一直往上找到
     `.top`（sticky）作包含块——于是它落在 `.navbar` 的横向滚动容器**之外**，不被裁剪，
     把文档的可滚动溢出区撑到它的右缘：移动端整页因此比视口宽（430 视口 → 446/458），
     并连带一条页面级横向滚动条。定位后标签被关在 `.proj` 里，与滚动容器一起裁剪。 */
  .proj {
    position: relative;
    margin-left: auto;
    flex: none;
  }
  .proj select {
    background: var(--input);
    border: 2px solid var(--pane);
    border-radius: 0;
    color: var(--text-2);
    font-size: 12px;
    padding: 2px 6px;
  }

  /* ── 待办计数入口：交互逐字不变（决策 92），仅换材质 ── */
  .pending-wrap {
    position: relative;
    flex: none;
    margin-left: 10px;
  }
  .pending-count {
    color: var(--text-3);
  }
  .dropdown {
    position: absolute;
    top: 34px;
    right: 0;
    width: 360px;
    max-height: 60vh;
    overflow: auto;
    padding: 6px;
    z-index: 30;
  }
  .dd-empty {
    padding: 10px 12px;
    color: var(--text-3);
    font-size: 12px;
  }
  .dd-item {
    display: grid;
    grid-template-columns: 10px 1fr;
    gap: 2px 8px;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
    color: var(--text);
  }
  .dd-item:hover {
    background: var(--wash);
  }
  .dd-dot::before {
    content: '!';
    color: var(--pending);
    font-size: 12px;
  }
  .dd-title {
    color: var(--text-hi);
    font-size: 12px;
  }
  .dd-msg {
    grid-column: 2;
    color: var(--text-3);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .btn-new {
    flex: none;
    margin-left: 10px;
    white-space: nowrap;
  }

  /* ── 移动款：铭牌行 + 信号灯缩略条 + 道具栏横滚行 + 页面导航行（顶栏 ≈ 138px） ── */
  @media (max-width: 479px) {
    .topbar {
      display: block;
      height: auto;
      padding: 0;
    }
    .bar-top {
      display: flex;
      align-items: center;
      gap: 8px;
      height: 44px;
      padding: 0 12px;
      border-bottom: 2px solid var(--hairline);
    }
    .railnav {
      display: flex;
      align-items: center;
      flex: 1;
      min-width: 0;
      margin-left: 2px;
    }
    .rn {
      position: relative;
      flex: 1;
      min-width: 0;
      height: 44px;
      display: grid;
      place-items: center;
    }
    /* 链节底纹：4px 亮 / 4px 暗的硬边像素条（非平滑渐变） */
    .rn::before {
      content: '';
      position: absolute;
      left: 0;
      right: 0;
      top: 50%;
      height: 4px;
      margin-top: -2px;
      background: repeating-linear-gradient(90deg, var(--pane) 0 4px, transparent 4px 8px);
    }
    .rn .mk {
      position: relative;
      z-index: 1;
      width: 10px;
      height: 10px;
      border: 2px solid var(--pane);
      background: var(--bg);
    }
    .rn.don .mk {
      background: var(--done);
      border-color: var(--done);
    }
    .rn.live .mk {
      background: var(--go);
      border-color: var(--go);
    }
    .rn.pen .mk {
      background: var(--pending);
      border-color: var(--pending);
    }
    .rn.on {
      outline: 2px solid var(--text-hi);
      outline-offset: -2px;
    }

    .filters-row {
      display: flex;
      align-items: center;
      gap: 6px;
      /* 3px 上下留白：行高由 44px 触控目标决定 → 50px；顶栏总高
         = 44(铭牌行) + 50(道具栏行) + 42(页面导航行) + 2(边框) ≈ 138px（§5） */
      padding: 3px 12px;
      overflow-x: auto;
      scrollbar-width: none;
    }
    .filters-row::-webkit-scrollbar {
      display: none;
    }
    .slots {
      flex: none;
      margin-left: 0;
      padding: 0;
      overflow: visible;
    }
    .slot {
      margin-left: 0;
    }
    .pending-wrap {
      margin-left: 2px;
    }
    .btn-new {
      flex: none;
      margin-left: 2px;
    }
    /* 下拉脱离横滚容器的裁剪，改用视口定位；顶栏 138px → top 146px */
    .dropdown {
      position: fixed;
      left: 12px;
      right: 12px;
      top: 146px;
      width: auto;
      max-height: 55vh;
      background: var(--bg);
    }
    .navbar {
      padding: 0 12px 8px;
    }
    .navbar .chip {
      min-height: 34px;
    }
    .proj {
      margin-left: 6px;
    }
    .proj select {
      min-height: 34px;
    }
  }
</style>
