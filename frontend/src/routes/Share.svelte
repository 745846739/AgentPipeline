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
  <header class="head">
    <h1 class="cond">手机访问</h1>
  </header>

  {#if loading}
    <div class="banner">正在读取服务地址…</div>
  {:else if error}
    <div class="banner error">{error}</div>
  {:else if info}
    {#if info.loopback_only}
      <section class="panel gate">
        <div class="gate-head cond">当前只绑定了本机回环地址</div>
        <p>
          手机和电脑不在同一个地址空间：<span class="mono">127.0.0.1</span> 在手机上指向手机自己，
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
          桌面应用默认也只绑回环；设环境变量 <span class="mono">AGENTPIPELINE_LAN=1</span>
          启动桌面壳即可开启局域网访问（决策 167）。
        </p>
        <p class="warn">
          ⚠ 服务能触发真实 LLM 调用并读取全部会话，开放局域网前请确认所在网段可信
          （更稳妥可用 SSH 隧道 / Tailscale）。
        </p>
      </section>
    {:else if addresses.length === 0}
      <section class="panel gate">
        <div class="gate-head cond">没有找到可用的局域网地址</div>
        <p>
          服务已绑定 <span class="mono">{info.host}:{info.port}</span>，但网卡枚举没有返回
          可访问的 IPv4 地址。可能是终端缺少网络信息权限，或本机当前没有连上局域网。
        </p>
        <p class="note">
          可手动用本机局域网 IP 访问：<span class="mono">http://&lt;本机 IP&gt;:{info.port}</span>
        </p>
      </section>
    {:else}
      <section class="panel qr-panel">
        <div class="qr-head">
          <span class="cond">扫码在手机上打开</span>
          <span class="mono badge">:{info.port}</span>
        </div>
        <div class="qr-stage">
          <!-- QR 由后端渲染（决策 167）：前端不引 QR 库；服务端只接受本服务地址 -->
          <img
            class="qr"
            src={qrSvgUrl(selected ?? addresses[0].url)}
            alt="扫码访问 {selected ?? addresses[0].url}"
            width="240"
            height="240"
          />
          <div class="qr-side">
            <div class="picked mono">{selected ?? addresses[0].url}</div>
            <button
              type="button"
              class="btn"
              onclick={() => copy(selected ?? addresses[0].url)}
            >
              {copied === (selected ?? addresses[0].url) ? '已复制' : '复制地址'}
            </button>
            <p class="hint">
              手机需与电脑在同一局域网（同一 Wi-Fi）。扫码后可直接使用看板与任务详情，
              实时进度经 SSE 推送。
            </p>
          </div>
        </div>

        {#if addresses.length > 1}
          <div class="alt">
            <div class="alt-head cond">其他网卡地址（首选不通时可换一个试试）</div>
            <ul class="alt-list">
              {#each addresses as a (a.url)}
                <li>
                  <button
                    type="button"
                    class="alt-item {a.url === selected ? 'on' : ''}"
                    onclick={() => (selected = a.url)}
                  >
                    <span class="iface">{a.interface}</span>
                    <span class="mono url">{a.url}</span>
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
      </section>
    {/if}
  {/if}
</div>

<style>
  .page {
    max-width: var(--detail-max);
    margin: 0 auto;
    padding: 24px 20px calc(48px + var(--safeb));
  }
  .crumb {
    font-size: 12px;
    color: var(--text-3);
    text-decoration: none;
  }
  .crumb:hover {
    color: var(--text-hi);
  }
  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    margin: 12px 0 18px;
  }
  .head h1 {
    font-family: var(--font-cond);
    font-size: 20px;
    color: var(--text-hi);
    margin: 0;
  }
  .banner {
    border: 1px solid var(--pane);
    background: var(--panel);
    color: var(--text-2);
    padding: 12px 14px;
    font-size: 12.5px;
  }
  .banner.error {
    border-color: var(--stop);
    color: var(--stop);
  }
  .panel {
    border: 1px solid var(--pane);
    background: var(--panel);
  }
  .cond {
    font-family: var(--font-cond);
    color: var(--text-hi);
    font-size: 13px;
  }
  .mono {
    font-family: var(--font-mono);
  }
  .qr-panel {
    padding: 20px;
  }
  .qr-head {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 16px;
  }
  .badge {
    color: var(--text-3);
    font-size: 11.5px;
    border: 1px solid var(--pane);
    padding: 2px 6px;
  }
  .qr-stage {
    display: flex;
    gap: 20px;
    align-items: flex-start;
    flex-wrap: wrap;
  }
  /* 二维码底色恒为浅色：深色主题下反色二维码识别率差（扫描器依赖明暗对比） */
  .qr {
    background: #fff;
    padding: 10px;
    border: 1px solid var(--pane);
    flex: 0 0 auto;
  }
  .qr-side {
    flex: 1 1 260px;
    min-width: 220px;
  }
  .picked {
    font-size: 13px;
    color: var(--text-hi);
    background: var(--input);
    border: 1px solid var(--pane);
    padding: 8px 10px;
    margin-bottom: 10px;
    word-break: break-all;
  }
  .btn {
    background: var(--go);
    color: var(--go-ink);
    border: 1px solid var(--go);
    font-family: var(--font-ui);
    font-size: 12px;
    padding: 6px 12px;
    cursor: pointer;
  }
  .btn:hover {
    background: var(--go-hi);
  }
  .hint {
    color: var(--text-3);
    font-size: 12px;
    line-height: 1.7;
    margin: 12px 0 0;
  }
  .alt {
    margin-top: 20px;
    border-top: 1px solid var(--hairline);
    padding-top: 14px;
  }
  .alt-head {
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 8px;
  }
  .alt-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .alt-item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    text-align: left;
    background: transparent;
    border: 1px solid transparent;
    color: var(--text);
    padding: 6px 8px;
    cursor: pointer;
    font-size: 12px;
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
  }
  .url {
    flex: 1;
    word-break: break-all;
  }
  .tag {
    color: var(--go);
    border: 1px solid var(--go);
    font-size: 11px;
    padding: 1px 5px;
  }
  .gate {
    padding: 20px;
    line-height: 1.8;
    color: var(--text-2);
    font-size: 12.5px;
  }
  .gate-head {
    font-size: 14px;
    margin-bottom: 12px;
    color: var(--pending);
  }
  .gate pre {
    background: var(--input);
    border: 1px solid var(--pane);
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
    margin-top: 18px;
    border-top: 1px solid var(--hairline);
    padding-top: 12px;
    color: var(--pending);
    font-size: 12px;
    line-height: 1.8;
  }

  /* 窄屏（<480px）：二维码与说明改纵向堆叠（theme-3 §8） */
  @media (max-width: 479px) {
    .page {
      padding: 16px 12px calc(48px + var(--safeb));
    }
    .qr-stage {
      flex-direction: column;
      align-items: stretch;
    }
    .qr {
      align-self: center;
    }
    .qr-side {
      min-width: 0;
    }
  }
</style>
