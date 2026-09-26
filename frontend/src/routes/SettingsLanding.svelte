<script lang="ts">
  import { router } from '../router.svelte';
  import { onHostMachine } from '../lib/localPage';

  /**
   * 设置落地页（`#/settings`，`route.name === 'settings-landing'`；决策 198 / design §4.3）。
   *
   * **分类法按用途三分**——「谁能进来」（入站边界）、「怎么找到你」（出站叫醒，
   * 决策 272⑧ 随离线通知落的第三类）、「怎么跑」（跑起来靠什么）。分类的一句话与
   * 每一项的一句话都是定稿文案（design §4.3 的表），逐字照抄别改。
   *
   * **各项仍是独立路由，落地页只是入口**：这里不复制任何设置内容、不内嵌表单、
   * 不替子页保存状态——它是一张门牌，点进去才是那一页。指标**不列**在这里：
   * 它不是设置，留在顶栏（第一屏三项之一）。
   *
   * 「手机访问」那一项**只在跑服务的这台机器本机上渲染**（决策 190 / 198）：
   * 判据是**来源是否回环**（`onHostMachine()`，**不看视口宽度**），非本机来源**不渲染**
   * 这一项（不是禁用、不是留个空位）；非本机来源直接敲 `#/share` 仍得到那一页既有的
   * 「去电脑上打开」指引。规则与后果一个字没变，只是入口的位子从顶栏换到了这里。
   */

  interface Item {
    path: string;
    label: string;
    note: string;
  }

  interface Category {
    key: string;
    title: string;
    note: string;
    items: Item[];
  }

  const CATEGORIES: Category[] = [
    {
      key: 'who',
      title: '谁能进来',
      note: '哪些仓库算工作对象、哪些设备能连进来。',
      items: [
        { path: '/settings/projects', label: '项目', note: '把本地仓库接进来当工作对象。' },
        {
          path: '/share',
          label: '手机访问',
          note: '让同一局域网里的手机连进来（只在跑服务的这台电脑上配置）。',
        },
      ],
    },
    {
      // 第三类（决策 272⑧）：「离线通知」是**出站**——这台机器主动找你，两头都不沾
      // 既有两类的判据（入站边界 / 运行时），硬塞进去会让分类那句话变成假话。
      key: 'reach',
      title: '怎么找到你',
      note: '这台机器主动把动静送到哪里——没人盯着浏览器时也找得到你。',
      items: [
        {
          path: '/settings/notify',
          label: '离线通知',
          note: '任务待办与失败、值班长回话完成时，往手机或机器人送信。',
        },
      ],
    },
    {
      key: 'how',
      title: '怎么跑',
      note: '跑起来用谁的能力、按什么规矩。',
      items: [
        {
          path: '/settings/foreman',
          label: '值守轮',
          note: '夜班值守的开关与节奏——关掉后今晚不会再自己醒。',
        },
        {
          path: '/settings/providers',
          label: '模型与密钥',
          // `台账` 本页首现，按决策 200② 的定稿说法给一次平实解释（同一页面内不重复）。
          note: '配 provider 台账（设置这一类页面）与密钥。',
        },
        {
          path: '/settings/stages',
          label: '阶段配置',
          note: '每个阶段用哪个 provider、带哪些工具与技能。',
        },
        { path: '/settings/market', label: '技能市场', note: '从 GitHub 仓装技能、看已装技能。' },
      ],
    },
  ];

  /** 本机判据只在装载时取一次：主机名在一次会话里不会变（决策 190）。 */
  const onHost = onHostMachine();

  /** 实际渲染的分类：本机之外的来源不给「手机访问」那一项（其余项一个不少）。 */
  const categories: Category[] = CATEGORIES.map((cat) => ({
    ...cat,
    items: cat.items.filter((item) => item.path !== '/share' || onHost),
  }));
</script>

<main class="page">
  <div class="p-head">
    <h1 class="p-title">设置</h1>
  </div>
  <p class="hintline">这台机器上的流水线怎么跑、谁能进来。</p>

  {#each categories as cat (cat.key)}
    <section class="cat" aria-labelledby={`cat-${cat.key}`}>
      <!-- 分类标题与各子页的小节标题**同档**（12px 字阶），不与页面标题（24px）同级（票 09） -->
      <h2 class="cat-title cond" id={`cat-${cat.key}`}>{cat.title}</h2>
      <p class="cat-note">{cat.note}</p>
      <ul class="items">
        {#each cat.items as item (item.path)}
          <li class="item">
            <a
              class="item-link"
              href="#{item.path}"
              onclick={() => router.navigate(item.path)}
            >
              <span class="item-name">{item.label}</span>
              <span class="item-note">{item.note}</span>
            </a>
          </li>
        {/each}
      </ul>
    </section>
  {/each}
</main>

<style>
  .page {
    max-width: var(--detail-max);
    margin: 0 auto;
    padding: 20px 24px 60px;
  }
  .cat {
    margin-bottom: 22px;
  }
  .cat-title {
    font-size: 12px;
    color: var(--text-hi);
    margin-bottom: 4px;
  }
  .cat-note {
    color: var(--text-3);
    line-height: 1.8;
    margin-bottom: 8px;
    max-width: 86ch;
  }
  /* 项清单 = 台账盒语汇：2px 描边、行间 --wash 分隔、零圆角（§3.1 的台账基元） */
  .items {
    list-style: none;
    border: 2px solid var(--pane);
    background: var(--bg);
  }
  .item + .item {
    border-top: 2px solid var(--wash);
  }
  .item-link {
    display: block;
    padding: 10px 12px;
    color: var(--text);
  }
  .item-link:hover {
    background: var(--panel);
    text-decoration: none;
  }
  .item-name {
    display: block;
    color: var(--text-hi);
  }
  .item-note {
    display: block;
    color: var(--text-3);
    line-height: 1.8;
    margin-top: 2px;
    max-width: 86ch;
  }
</style>
