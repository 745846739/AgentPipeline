<script lang="ts">
  import { board, FILTER_LABELS, type StatusFilter } from '../../stores/board.svelte';
  import { router } from '../../router.svelte';
  import NewTaskDialog from '../board/NewTaskDialog.svelte';
  import Sprite from '../render/Sprite.svelte';
  import { BOARD_COLUMNS, columnForTask } from '../../lib/pipeline';
  import type { SpriteName } from '../../theme/contract';
  import { tick } from 'svelte';

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

  /**
   * 页面导航行**三项**（决策 198 / design §4.2）：对讲台 / 指标 / 设置。
   *
   * 这是**有意的收缩**——顶栏是「第一屏必须懂」的那一处，原先六项等于让人先学词表再开始用。
   * 原「项目 / 模型与密钥 / 技能市场 / 手机访问」四项**从顶栏移入设置落地页**（项名逐字不改），
   * 各自路由不变；收缩**只针对这一行**，顶栏其余部分（wordmark / 项目切换器 / 道具栏过滤槽 /
   * 「待处理 N」芯片 / 「新建任务」）一字不动——它们不是导航项。
   *
   * `routes` 是**高亮判据的集合**（design §4.2 的第三列）：「设置」在落地页与五个设置类页面
   * （含 `/share`）六处都点亮。
   */
  const NAV: Array<{ path: string; routes: string[]; label: string; sprite: SpriteName }> = [
    // 对讲台（决策 174 / theme-6-pixel.md §3.3）：与看板并列，故排在台账页之前。
    // 图元复用既有 foreman 头像（不新增 sprite）；它在 34px 页签盒里按 16px 显示。
    { path: '/talk', routes: ['talk'], label: '对讲台', sprite: 'foreman' },
    { path: '/metrics', routes: ['metrics'], label: '指标', sprite: 'chart' },
    {
      path: '/settings',
      routes: [
        'settings-landing',
        'settings-projects',
        'settings-providers',
        'settings-stages',
        'settings-market',
        // 手机访问（决策 167 / 186）也是设置类页面：它现在**只从落地页进**
        // （「只在本机给入口」的判据在 `SettingsLanding.svelte`，与决策 190 同源）。
        'share',
      ],
      label: '设置',
      sprite: 'key',
    },
  ];

  let newTaskOpen = $state(false);

  /**
   * 顶栏**实测**高度（含 2px 下框），写进 `document.documentElement` 的 `--topbar-h`（票 09）。
   *
   * 为什么不能写死：顶栏在窄档会折成两行而更高（桌面 78–81px，移动款约 138px），
   * 而钉在它下面的东西（详情页的档案盒）要按**这一档的真实值**让位——写死一个数，
   * 换一档就错（56px 那个旧值就是这么把「等你拍板」铭牌送到顶栏底下的）。
   * 用 `offsetHeight` 而不是 `clientHeight`：后者不含边框，而压在顶栏下沿的那 2px 框
   * 也是「被盖住」的一部分。
   */
  let topbarH = $state(0);
  $effect(() => {
    if (typeof document === 'undefined') return;
    const h = topbarH;
    if (h > 0) document.documentElement.style.setProperty('--topbar-h', `${h}px`);
  });

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

  /* ─────────────── 待处理下拉：键盘与关闭（票 04 / R2-04） ───────────────
   *
   * 它原来播报自己是 `role="menu"`，却没有菜单的**任何**行为：Escape 关不掉、方向键不动、
   * 点面板外面也不关，触发钮还缺 `aria-controls`。实测 `afterEsc:1 / afterOutside:1`。
   *
   * **选的路：降级成普通弹层**（去掉 `role=menu` / `role=menuitem`）。理由：这一栏里每一行
   * 都是「跳到某个任务详情」的**链接**，链接不是菜单项。补齐 `role=menu` 的契约要求把行播报
   * 成「菜单项」——那正好丢掉链接语义，而票面边界明令不能为了菜单语义改掉导航语义。
   * 降级之后行为上补齐三条出口，并把方向键也接上（可选的自在性，不是菜单契约）。
   */
  let pendingWrap = $state<HTMLDivElement | null>(null);
  let pendingTrigger = $state<HTMLButtonElement | null>(null);
  let pendingPanel = $state<HTMLDivElement | null>(null);

  function pendingItems(): HTMLAnchorElement[] {
    return pendingPanel ? [...pendingPanel.querySelectorAll<HTMLAnchorElement>('a.dd-item')] : [];
  }

  function focusPendingItem(index: number): void {
    const list = pendingItems();
    if (list.length === 0) return;
    const n = list.length;
    list[((index % n) + n) % n].focus();
  }

  function closePending(returnFocus: boolean): void {
    board.pendingOpen = false;
    if (returnFocus) pendingTrigger?.focus();
  }

  /**
   * 键盘一律在 `window` 上收（触发钮与面板都不挂 `onkeydown`：那两个落点要么给静态元素挂
   * 交互处理器、要么把按钮的默认语义扯歪，两条都是 a11y 检查里的红灯）。判据收紧到
   * ——面板开着，且焦点在触发钮或面板里。
   */
  function onWindowKey(e: KeyboardEvent): void {
    const active = document.activeElement as HTMLElement | null;
    const onTrigger = !!pendingTrigger && active === pendingTrigger;
    const inPanel = !!active && !!pendingPanel && pendingPanel.contains(active);

    // 触发钮上按 ArrowDown：打开并把焦点送进第一项（面板刚变可见，要等一次 DOM 刷新）
    if (e.key === 'ArrowDown' && onTrigger && !board.pendingOpen) {
      e.preventDefault();
      board.togglePendingDropdown();
      void tick().then(() => focusPendingItem(0));
      return;
    }
    if (!board.pendingOpen) return;

    // Escape 一律关得掉——**焦点从没进过面板时也算**（实测里就是这个场景：点开芯片、
    // 焦点还在芯片上按 Escape）。焦点若在触发钮或面板里，顺带还回去。
    if (e.key === 'Escape') {
      closePending(onTrigger || inPanel);
      return;
    }
    if (!onTrigger && !inPanel) return;

    const list = pendingItems();
    if (list.length === 0) return;
    const current = list.indexOf(active as HTMLAnchorElement);
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      focusPendingItem(current + 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (current <= 0) pendingTrigger?.focus();
      else focusPendingItem(current - 1);
    } else if (e.key === 'Home') {
      e.preventDefault();
      focusPendingItem(0);
    } else if (e.key === 'End') {
      e.preventDefault();
      focusPendingItem(list.length - 1);
    }
  }

  /** 点面板外面关掉（含「本来就开着、用户去点别处」那一档）。 */
  function onWindowClick(e: MouseEvent): void {
    if (!board.pendingOpen) return;
    const target = e.target as Node | null;
    if (target && pendingWrap?.contains(target)) return;
    board.pendingOpen = false;
  }

  /**
   * 点信号灯缩略条跳段（移动原型 `.rn` + `scrollIntoView`）。
   * 站点带带 `scroll-margin-top: 148px`，故跳到顶时不会被 138px 的顶栏压住。
   *
   * **没有靶子时先去有靶子的那一页**（决策 218 ⑥）：`#s-<key>` 只存在于看板
   * （`BoardColumn.svelte`），故在对讲台这类页面上点灯以前是**一动不动**的——而它自报
   * `aria-label="跳到 <列名>"`，接通是兑现承诺、不是加功能。改址走**路由跳转**
   * （`router.navigate`，hash 变化不整页刷新），等一次 DOM 刷新再定位——用 `tick()`
   * 而不是定时器（后者是「等得够久就成了」的赌博，而且会与路由的渲染节奏错位）。
   */
  function jumpToStation(key: string) {
    if (typeof document === 'undefined') return;
    const here = document.getElementById(`s-${key}`);
    if (here) {
      here.scrollIntoView({ block: 'start' });
      return;
    }
    router.navigate('/');
    void tick().then(() =>
      document.getElementById(`s-${key}`)?.scrollIntoView({ block: 'start' }),
    );
  }
</script>

<svelte:window onclick={onWindowClick} onkeydown={onWindowKey} />

<header class="top" bind:offsetHeight={topbarH}>
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
          <!-- 图标 + 词（决策 201）：屏幕上的字才是「一眼扫过去就懂」的那一层，title 与
               aria-label 只作辅助。第 7 槽标签位用短式「已结束」——34px 行放不下 7 个字，
               完整式「已结束（失败·取消）」放 title。计数徽章：第 3 槽**不画**（`countFor('pending')`
               与紧邻的 `pendingCount` 是同一个数，数由「待处理 N」芯片唯一承载），其余六槽保留。 -->
          {@const label = f === 'ended' ? '已结束' : FILTER_LABELS[f]}
          <button
            type="button"
            class="slot {board.filter === f ? 'on' : ''} {f === 'pending' ? 'pend' : ''}"
            title={FILTER_LABELS[f]}
            aria-label="{FILTER_LABELS[f]}（{board.countFor(f)}）"
            aria-pressed={board.filter === f}
            onclick={() => board.setFilter(f)}
          >
            <Sprite name={SLOT_SPRITES[f]} />
            <span class="lbl">{label}</span>
            {#if f !== 'pending'}
              <span class="cb">{board.countFor(f)}</span>
            {/if}
          </button>
        {/each}
      </nav>

      <div class="pending-wrap" bind:this={pendingWrap}>
        <button
          type="button"
          class="chip pending-count {board.pendingCount > 0 ? 'pend' : ''}"
          aria-expanded={board.pendingOpen}
          aria-controls="pending-dropdown"
          bind:this={pendingTrigger}
          onclick={() => board.togglePendingDropdown()}
        >
          待处理 <span class="c">{board.pendingCount}</span>
        </button>
        <!-- 面板**常驻 DOM**、靠 `hidden` 开合（票 04）：`aria-controls` 指过去的目标必须真的
             存在，IDREF 悬空是另一条会烂掉的账。 -->
        <div
          class="dropdown panel"
          id="pending-dropdown"
          hidden={!board.pendingOpen}
          bind:this={pendingPanel}
        >
          {#if board.pendingTasks.length === 0}
            <div class="dd-empty">当前没有待办任务。</div>
          {:else}
            {#each board.pendingTasks as task (task.id)}
              <!-- 每一行是一个**链接**（跳任务详情），不是菜单项——这正是选降级而不是补齐
                   `role=menu` 的理由：菜单项会盖掉链接语义。 -->
              <a class="dd-item" href="#/task/{task.id}" onclick={() => openTask(task.id)}>
                <span class="dd-dot"></span>
                <span class="dd-title">{task.title}</span>
                <span class="dd-msg">{task.pending_reason?.message ?? ''}</span>
              </a>
            {/each}
          {/if}
        </div>
      </div>

      <button type="button" class="btn btn-new" onclick={() => (newTaskOpen = true)}>新建任务</button>
    </div>
  </div>

  <nav class="navbar" aria-label="页面导航">
    {#each NAV as item (item.path)}
      <a
        href="#{item.path}"
        class="chip navchip"
        class:on={item.routes.includes(router.route.name)}
        aria-current={item.routes.includes(router.route.name) ? 'page' : undefined}
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

  /* ── 道具栏槽位：34px 高、2px 描边、图标 + 词 + 右下角计数徽章、选中 = 亮描边 + wash 底 ──
     宽随词走（图元 16px + 4px 间距 + 12px 词），**高度仍恒 34px**（决策 201）；
     待处理槽不画计数徽章（数在紧邻的「待处理 N」芯片上）。 */
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
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    min-width: 34px;
    height: 34px;
    padding: 0 6px;
    margin-left: -2px;
    border: 2px solid var(--pane);
    background: var(--panel);
    color: var(--text-2);
  }
  .slot:first-child {
    margin-left: 0;
  }
  .slot .lbl {
    font-size: 12px;
    line-height: 1;
    white-space: nowrap;
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
  /* 选中芯片的前缀三角：剪影画（`clip-path`），一个字都不进可访问名（票 06 / R2-19） */
  .chip.on::before {
    content: '';
    display: inline-block;
    width: 10px;
    height: 9px;
    background: var(--go);
    clip-path: polygon(0 0, 100% 50%, 0 100%);
    margin-right: 5px;
    vertical-align: -1px;
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
  /* 常驻 DOM + `hidden` 开合（票 04）：显式写出来，免得将来哪条 `display` 规则把它顶掉 */
  .dropdown[hidden] {
    display: none;
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
    text-decoration: none;
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
         = 44(铭牌行) + 50(道具栏行) + 42(页面导航行) + 2(边框) ≈ 138px（§5）。
         **这一行自己不再横滚**（决策 201）：横滚收进槽位行（`.slots`），于是紧邻的
         「待处理 N」与「新建任务」恒定完整可见——它们不再随槽位横滚出屏。 */
      padding: 3px 12px;
      overflow: visible;
    }
    .slots {
      /* 槽位行照旧横滚：可伸缩（min-width: 0）+ 自己滚，滚动条照旧隐藏 */
      flex: 1 1 auto;
      min-width: 0;
      margin-left: 0;
      /* 与桌面同值：给计数徽章的 -5px 溢出留出裁剪余量（滚动容器的裁剪边是内边距盒） */
      padding: 5px 6px 5px 0;
      overflow-x: auto;
    }
    /* 窄屏（<480px）**只给当前选中槽带词**，其余六槽保持 34px 图标槽（决策 201） */
    .slot .lbl {
      display: none;
    }
    .slot.on .lbl {
      display: inline;
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
