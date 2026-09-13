<script lang="ts">
  import { onMount } from 'svelte';
  import { getServerInfo, qrSvgUrl } from '../api/client';
  import type { ServerAddress, ServerInfo } from '../api/types';

  /**
   * 局域网分享页（决策 167）：手机扫码接入。
   *
   * 页面只做一件事——把「手机能连上的地址」变成一个可扫的二维码。地址由后端
   * 枚举网卡得出（crates/app/src/lan.rs），前端不做任何猜测：多网卡 / VPN 环境下
   * 选错地址的表现是「扫了打不开」，故这里把后端排好序的推荐项放大，其余列为备选。
   *
   * 仅回环绑定时不显示二维码（拷给手机也连不上），改为给出开启局域网访问的指引。
   * 二维码由**后端渲染** SVG（决策 167），前端不引 QR 库。
   */

  let info = $state<ServerInfo | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  /** 当前选中用于生成二维码的地址（缺省取后端推荐的首项）。 */
  let selected = $state<string | null>(null);
  let copied = $state<string | null>(null);

  const addresses = $derived(info?.addresses ?? []);

  onMount(async () => {
    try {
      info = await getServerInfo();
      selected = addresses[0]?.url ?? null;
    } catch (err) {
      error = (err as Error).message;
    } finally {
      loading = false;
    }
  });

  async function copy(url: string) {
    try {
      await navigator.clipboard.writeText(url);
      copied = url;
      // 2s 后清掉「已复制」态，避免长时间占位误导
      setTimeout(() => {
        if (copied === url) copied = null;
      }, 2000);
    } catch {
      // 非安全上下文（http 且非 localhost）下 clipboard 不可用：
      // 不报错打断，用户仍可手动选中二维码下方的地址文本
      copied = null;
    }
  }

  /** 首选地址排在首位时高亮；用户手动切换后仍按后端 preferred 标记着色。 */
  function isPreferred(a: ServerAddress) {
    return a.preferred;
  }
</script>

<div class="page">
  <a class="crumb" href="#/">← 看板</a>
  <div class="p-head">
    <h1 class="p-title">手机访问</h1>
  </div>

  {#if loading}
    <div class="banner">正在读取服务地址…</div>
  {:else if error}
    <div class="banner error">{error}</div>
  {:else if info}
    {#if info.loopback_only}
      <section class="gate">
        <div class="gate-head">仅回环绑定时</div>
        <p>
          手机和电脑不在同一个地址空间：<b>127.0.0.1</b> 在手机上指向手机自己，
          扫码必然打不开。要让手机访问，需要让服务监听局域网网卡。
        </p>
        <p class="how">命令行启动时绑定全网卡：</p>
        <pre><code>agent-pipeline serve --host 0.0.0.0</code></pre>
        <p class="how">或在配置文件里改：</p>
        <pre><code>[server]
host = "0.0.0.0"</code></pre>
        <p class="note">
          手机加载的页面与 API <span class="hi">同源</span>，因此无需额外放行 origin
          （跨源防护只拦异源写请求，同源写请求自带客户端头，决策 128 / 153③）。
        </p>
        <p class="note">
          桌面应用默认也只绑回环；设环境变量 <b>AGENTPIPELINE_LAN=1</b>
          启动桌面壳即可开启局域网访问（决策 167）。
        </p>
        <p class="warn">
          ⚠ 服务能触发真实 LLM 调用并读取全部会话，开放局域网前请确认所在网段可信
          （更稳妥可用 SSH 隧道 / Tailscale）。
        </p>
      </section>
    {:else if addresses.length === 0}
      <section class="gate">
        <div class="gate-head">没有找到可用的局域网地址</div>
        <p>
          服务已绑定 <span class="mono">{info.host}:{info.port}</span>，但网卡枚举没有返回
          可访问的 IPv4 地址。可能是终端缺少网络信息权限，或本机当前没有连上局域网。
        </p>
        <p class="note">
          可手动用本机局域网 IP 访问：<span class="mono">http://&lt;本机 IP&gt;:{info.port}</span>
        </p>
      </section>
    {:else}
      <div class="qrbox">
        <!-- 二维码底盒恒白：扫描器依赖明暗对比，浅色主题也不例外（§3.1） -->
        <div class="qr-qr">
          <img
            src={qrSvgUrl(selected ?? addresses[0].url)}
            alt="扫码访问 {selected ?? addresses[0].url}"
            width="240"
            height="240"
          />
        </div>
        <div class="qr-side">
          <div class="chart-head">
            <span class="reg-name">扫码在手机上打开</span>
            <span class="port mono">:{info.port}</span>
          </div>
          <div class="picked mono">{selected ?? addresses[0].url}</div>
          <button
            type="button"
            class="btn"
            onclick={() => copy(selected ?? addresses[0].url)}
          >
            {copied === (selected ?? addresses[0].url) ? '已复制' : '复制地址'}
          </button>
          <p class="qr-cap">
            手机需与电脑在同一局域网（同一 Wi-Fi）。扫码后可直接使用看板与任务详情，
            实时进度经 SSE 推送。
          </p>
        </div>
      </div>

      {#if addresses.length > 1}
        <div class="alt">
          <div class="alt-head">其他网卡地址（首选不通时可换一个试试）</div>
          <ul class="alt-l">
            {#each addresses as a (a.url)}
              <li>
                <button
                  type="button"
                  class="alt-item {a.url === selected ? 'on' : ''}"
                  onclick={() => (selected = a.url)}
                >
                  <span class="iface">{a.interface}</span>
                  <span class="mono u">{a.url}</span>
                  {#if isPreferred(a)}<span class="tag">推荐</span>{/if}
                </button>
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      <p class="warn">
        ⚠ 同一网段的任何设备都能访问本服务的全部接口（v1 无鉴权），请勿在公共
        Wi-Fi 下开启。用完可重启服务回到仅回环绑定。
      </p>
    {/if}
  {/if}
</div>

<style>
  .page {
    max-width: var(--detail-max);
    margin: 0 auto;
    padding: 20px 24px calc(48px + var(--safeb));
  }
  .banner {
    border: 2px solid var(--pane);
    background: var(--panel);
    color: var(--text-2);
    padding: 12px 14px;
    font-size: 12px;
  }
  .banner.error {
    border-color: var(--stop);
    color: var(--stop);
  }
  /* 二维码牌：2px 描边盒 + 右侧地址栏（移动款纵向堆叠，见下方媒体查询） */
  .qrbox {
    display: flex;
    gap: 20px;
    align-items: flex-start;
    flex-wrap: wrap;
    border: 2px solid var(--pane);
    background: var(--panel);
    padding: 16px;
  }
  /* 二维码底色恒白：深色主题下反色二维码识别率差（扫描器依赖明暗对比）。
     这是 css-parity.test.ts 唯一 allowlist 的裸 #fff。 */
  .qr-qr {
    flex: none;
    width: 240px;
    height: 240px;
    background: #fff;
    border: 2px solid var(--pane);
    display: grid;
    place-items: center;
  }
  .qr-qr img {
    display: block;
  }
  .qr-side {
    flex: 1;
    min-width: 260px;
  }
  .chart-head {
    display: flex;
    align-items: baseline;
    gap: 10px;
    margin-bottom: 10px;
  }
  .port {
    color: var(--text-3);
    border: 2px solid var(--pane);
    padding: 0 5px;
  }
  .picked {
    background: var(--bg);
    border: 2px solid var(--pane);
    color: var(--text-hi);
    padding: 8px 10px;
    margin-bottom: 10px;
    word-break: break-all;
  }
  .qr-cap {
    margin-top: 12px;
    color: var(--text-3);
    line-height: 1.8;
  }
  .alt {
    margin-top: 18px;
    border-top: 2px solid var(--wash);
    padding-top: 12px;
  }
  .alt-head {
    color: var(--text-3);
    letter-spacing: 0.08em;
    margin-bottom: 8px;
  }
  .alt-l {
    list-style: none;
  }
  .alt-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    text-align: left;
    border: 2px solid transparent;
    color: var(--text);
    padding: 3px 8px;
  }
  .alt-item:hover {
    background: var(--wash);
  }
  .alt-item.on {
    border-color: var(--go);
  }
  .iface {
    color: var(--text-3);
    min-width: 56px;
    flex: none;
  }
  .u {
    flex: 1;
    word-break: break-all;
  }
  .tag {
    flex: none;
    color: var(--go);
    border: 2px solid var(--go);
    padding: 0 5px;
  }
  /* 仅回环绑定 / 无地址：琥珀标题的指引块（决策 167） */
  .gate {
    border: 2px solid var(--pane);
    background: var(--panel);
    padding: 16px;
    line-height: 1.85;
    color: var(--text-2);
  }
  .gate-head {
    color: var(--pending);
    font-size: 24px;
    margin-bottom: 10px;
  }
  .gate pre {
    background: var(--bg);
    border: 2px solid var(--pane);
    padding: 10px 12px;
    overflow-x: auto;
    margin: 8px 0 14px;
  }
  .gate code {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-hi);
  }
  .how {
    color: var(--text-3);
    margin: 12px 0 4px;
  }
  .note {
    margin-top: 14px;
    color: var(--text-3);
  }
  .hi {
    color: var(--text-hi);
  }
  .warn {
    margin-top: 16px;
    border-top: 2px solid var(--wash);
    padding-top: 12px;
    color: var(--pending);
    line-height: 1.85;
  }

  /* 窄屏（<480px）：二维码与说明纵向堆叠；地址项改 44px 触控行（§5） */
  @media (max-width: 479px) {
    .page {
      padding: 16px 12px calc(48px + var(--safeb));
    }
    .qrbox {
      flex-direction: column;
      gap: 14px;
    }
    .qr-qr {
      align-self: center;
    }
    .qr-side {
      min-width: 0;
    }
    .alt-item {
      min-height: 44px;
      padding: 0 8px;
    }
  }
</style>
