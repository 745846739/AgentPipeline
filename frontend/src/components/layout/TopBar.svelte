<script lang="ts">
  import { board, FILTER_LABELS, type StatusFilter } from '../../stores/board.svelte';
  import { router } from '../../router.svelte';
  import NewTaskDialog from '../board/NewTaskDialog.svelte';
  import Sprite from '../render/Sprite.svelte';
  import type { SpriteName } from '../../theme/contract';
  import { createMenuTrap } from '../../lib/menuTrap';

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
   * 页面导航行**四项**（决策 240，修订决策 198）：对讲台 / 看板 / 指标 / 设置。
   *
   * 「看板」原先**不在这行里**——它是根路由 `#/`，入口散在 wordmark、各页面包屑
   * （`← 看板`）与空态（「回看板」）三处，于是「看板在哪儿进」取决于人当时站在哪一页。
   * 决策 240 把入口收成**这一行里的一枚页签**，与对讲台同排、同样式、同高亮规则
   * （用户选的顺序：紧贴对讲台之后）；其余三处入口随之摘除——**看板只从这里进**。
   *
   * 图元复用既有 sprite（不新增图元）：`chest`（货箱）= 看板上的任务卡。
   *
   * 决策 198 的收缩**照旧成立**（顶栏是「第一屏必须懂」的那一处，六项等于先学词表）：
   * 原先移入设置落地页的四项（项目 / 模型与密钥 / 技能市场 / 手机访问）**不回这一行**，
   * 各自路由不变。顶栏其余部分（wordmark / 项目切换器 / 道具栏过滤槽 / 「待处理 N」芯片 /
   * 「新建任务」）一字不动——它们不是导航项。
   *
   * `routes` 是**高亮判据的集合**（design §4.2 的第三列）：「设置」在落地页与**每一个设置类
   * 子页**（含 `/share`）都点亮——设置类页面各是一个独立路由，漏一个的表现是「从落地页点进去，
   * 顶栏那一枚当场灭掉」；「看板」只在根路由上点亮（任务详情不是这一行的项，
   * 与对讲台不在详情页点亮同理）。
   */
  const NAV: Array<{ path: string; routes: string[]; label: string; sprite: SpriteName }> = [
    // 对讲台（决策 174 / theme-6-pixel.md §3.3）：与看板并列，故排在台账页之前。
    // 图元复用既有 foreman 头像（不新增 sprite）；它在页签盒里按 16px 显示（宽窄两档同）。
    { path: '/talk', routes: ['talk'], label: '对讲台', sprite: 'foreman' },
    // 看板（决策 240）：根路由，与对讲台同排同款。
    { path: '/', routes: ['board'], label: '看板', sprite: 'chest' },
    { path: '/metrics', routes: ['metrics'], label: '指标', sprite: 'chart' },
    {
      path: '/settings',
      routes: [
        'settings-landing',
        'settings-projects',
        'settings-providers',
        'settings-stages',
        'settings-market',
        // 值守轮（287）与离线通知（272）、命令执行（297）都是设置类页面：它们**只从落地页进**
        // （「只在本机给入口」的判据在 `SettingsLanding.svelte`，与决策 190 同源）。
        // 这三条此前漏在这张表外面：从落地页点进去，顶栏那一枚「设置」会当场灭掉。
        'settings-foreman',
        'settings-notify',
        'settings-tools',
        // 手机访问（决策 167 / 186）同理。
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
   * 为什么不能写死：顶栏在窄档会折成多行而更高（桌面 78–81px；窄档**还按路由分两档**——
   * 导航行已移到屏幕底部（决策 243），顶栏只剩道具栏行且只在看板露出，故看板 52px、
   * **其余路由 0px**），而钉在它下面的东西（详情页的档案盒、完成横幅、看板跳段的
   * scroll-margin）要按**这一档的真实值**让位——写死一个数，换一档就错
   * （56px 那个旧值就是这么把「等你拍板」铭牌送到顶栏底下的）。
   * 用 `offsetHeight` 而不是 `clientHeight`：后者不含边框，而压在顶栏下沿的那 2px 框
   * 也是「被盖住」的一部分。
   *
   * **0 也是合法实测值**（决策 243：窄档非看板顶栏清零），不能用 `h > 0` 把它挡掉——
   * 否则变量会停在上一档的值（78 / 52），下游钉位全错。`bind:offsetHeight` 的回调
   * 尚未到达（或 ResizeObserver 不实现，如 jsdom）时，退回直接读一次实测高度。
   */
  let topbarH = $state(0);
  let headerEl = $state<HTMLElement | null>(null);
  $effect(() => {
    if (typeof document === 'undefined' || !headerEl) return;
    const h = topbarH || headerEl.offsetHeight;
    document.documentElement.style.setProperty('--topbar-h', `${h}px`);
  });

  const sessionName = $derived(
    board.projects.find((p) => p.id === board.projectId)?.name ?? 'AgentPipeline',
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

  /**
   * 键盘与点外关闭的**判据**在 `lib/menuTrap`（决策 251⑤）——⋯ 班次菜单与这一栏是同义的
   * 两份（各 80 多行、只换了标识符），共用一份之后抄漏陷阱就不再可能。这里只出**接线**：
   * 开关态住在 `board.pendingOpen`，而「打开」在这一栏顺带拉一次待办列表。
   *
   * 键盘一律在 `window` 上收（触发钮与面板都不挂 `onkeydown`：那两个落点要么给静态元素挂
   * 交互处理器、要么把按钮的默认语义扯歪，两条都是 a11y 检查里的红灯）——那两条监听仍在
   * 模板的 `<svelte:window>` 上，立场没挪地方。
   */
  const menuTrap = createMenuTrap({
    isOpen: () => board.pendingOpen,
    onOpen: () => board.togglePendingDropdown(),
    onClose: () => {
      board.pendingOpen = false;
    },
    trigger: () => pendingTrigger,
    panel: () => pendingPanel,
    wrap: () => pendingWrap,
    itemSelector: 'a.dd-item',
  });
</script>

<svelte:window onclick={menuTrap.onClick} onkeydown={menuTrap.onKeydown} />

<header class="top" bind:this={headerEl} bind:offsetHeight={topbarH}>
  <div class="topbar">
    <div class="bar-top">
      <span class="logo" aria-hidden="true"></span>
      <!-- wordmark（决策 240）：**铭牌，不是入口**。它原先是 `href="#/"` 的看板入口，而看板
           现在是这一行里的一枚页签（见 `NAV`）——同一个目的地两处入口，其中一处还是「看着像
           站名、点了却换页」的那种。留字、去链：它说的仍是这台机器的名字。 -->
      <span class="wordmark">AGENTPIPELINE</span>
      <span class="sess">0:{sessionName}</span>
    </div>

    <!-- 道具栏行**只跟看板相关**（状态过滤、待处理、新建任务都只在看板上有意义），
         故窄档只在看板路由露出（页面导航行已移到屏幕底部，决策 243）；
         桌面档照旧拍平进 `.topbar`。
         类名用 `on-board` 不用 `board`：看板页容器自己占着 `.board`（`Board.svelte:144`），
         撞名会让 `page.locator('.board')` 命中两个元素（实测 e2e strict mode violation）。 -->
    <div class="filters-row" class:on-board={router.route.name === 'board'}>
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
  /* 顶栏 = 车间铭牌（sticky）：桌面款第一行铭牌 + 道具栏，第二行页面铭牌排（§3）；
     窄档（≤479）导航行**移到屏幕底部**成页签栏、顶栏只留道具栏行（只在看板露出），
     见文件末的媒体查询（决策 243） */
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
  /* 工头头像的契约显示尺寸是 48px（dossier 用），塞进页签盒会把它撑高——窄档页签栏
     高是 `--nav-h` 账本里写死的数（app.css：58 + safeb），撑高即账实不符；导航处一律
     按 chip 节奏缩到 16px（规格 §3.3）。 */
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

  /* ── 移动款（决策 243）：**页面导航行搬出顶栏、钉到屏幕底缘**成底部页签栏
     （四项等分、图标在上、命中区 ≥44px、独占安全区）；铭牌行（logo / wordmark /
     会话名）整行去掉照旧；顶栏只剩道具栏行、且只在看板路由露出——
     看板 52px、**非看板路由 0px**（顶栏清零，内容全屏展开）。
     各处钉位一律读 `--topbar-h`（0 也是合法值）；底部让位读 `--sbar-h`
     （窄档 = 页签栏 `--nav-h` 一层——状态条已随决策 300 整条退场，此前是
     「状态条 42 + 页签栏」两层，见 app.css 账本）。 */
  @media (max-width: 479px) {
    .top {
      display: flex;
      flex-direction: column;
      /* 下框改由道具栏行自带（它只在看板露出）：非看板路由顶栏清零后不留 2px 残线 */
      border-bottom: none;
    }
    .topbar {
      display: block;
      height: auto;
      padding: 0;
    }
    .bar-top {
      display: none;
    }
    /* 道具栏行只跟看板相关：非看板路由整行不渲染（`.on-board` 由模板上的
       `class:on-board` 挂——**不能叫 `.board`**，那个类名是看板页容器的） */
    .filters-row {
      display: none;
    }
    .filters-row.on-board {
      display: flex;
      align-items: center;
      gap: 6px;
      /* 3px 上下留白 + 行高由 44px 触控目标决定（44 + 6）+ 2px 下框（顶栏的框挪进来）
         → 顶栏 = 52px；非看板路由这一行不露出 → 顶栏 0px。
         **这一行自己不横滚**（决策 201）：横滚收进槽位行（`.slots`），于是紧邻的
         「待处理 N」与「新建任务」恒定完整可见——它们不再随槽位横滚出屏。 */
      padding: 3px 12px;
      border-bottom: 2px solid var(--pane);
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
    /* 下拉脱离横滚容器的裁剪，改用视口定位；钉在顶栏下沿（顶栏高度按路由变，故读 `--topbar-h`） */
    .dropdown {
      position: fixed;
      left: 12px;
      right: 12px;
      top: calc(var(--topbar-h) + 8px);
      width: auto;
      max-height: 55vh;
      background: var(--bg);
    }

    /* ── 底部页签栏（决策 243）：脱出文档流、钉视口底缘。
       盒高 = 2(上框) + 6(上留白) + 44(页签) + 6(下留白) + safeb = 58 + safeb，
       与 app.css 里 `--nav-h` 的取值逐字对应——改这里必须同步改那笔账。 */
    .navbar {
      position: fixed;
      left: 0;
      right: 0;
      bottom: 0;
      z-index: 32;
      gap: 0;
      padding: 6px 6px calc(6px + var(--safeb));
      border-top: 2px solid var(--pane);
      background: var(--bg);
      /* 四项恒定放得下：等分整宽，不再横滚 */
      overflow: visible;
    }
    .navbar .chip {
      flex: 1 1 0;
      min-width: 0;
      /* 定高 44（border-box）：内容恰好 16(图) + 2(gap) + 18(行盒) = 36，
         加 2×2 内边距 + 2×2 框 = 44——`min-height` 在这里拦不住（内容自然高
         48 会把它顶掉），只有定死才能让页签栏盒高与 `--nav-h` 账本（58）对上。
         图标在上、文字在下（移动页签范式）；命中区 ≥44px 补齐移动基线。
         `overflow: hidden` 兜住行盒波动，绝不把整页撑出横向滚动 */
      height: 44px;
      flex-direction: column;
      justify-content: center;
      gap: 2px;
      padding: 2px;
      overflow: hidden;
    }
    /* 列排里前缀三角会悬在图标上方，且 wash 底 + 亮描边已足够表意——去掉 */
    .navbar .chip.on::before {
      display: none;
    }
    .navbar .chip .ic {
      margin-right: 0;
      vertical-align: baseline;
    }
    .proj {
      flex: none;
      margin-left: auto;
      /* label 是 inline 容器时，select（inline-block）的基线 descender 会把行盒撑高
         约 4px——页签栏盒高是 `--nav-h` 账本里写死的 58，一行都不许多。
         改 flex 后行盒间隙消失，select 由自身 `height` 定高。 */
      display: flex;
      align-items: center;
    }
    .proj select {
      /* 定高（border-box 含 2px 框 = 44 触控底线）：intrinsic 高度不得上撑页签栏 */
      height: 44px;
      max-width: 34vw;
    }
  }
</style>
