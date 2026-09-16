import type { ServerInfo } from '../api/types';
import { isLoopbackHostname } from './localPage';

/**
 * 「绑定全网卡 / 只绑本机」两颗钮的判据（决策 186）。
 *
 * **为什么不是一句 `await setLan(); await refresh()`**：改绑会切断当前所有连接，
 * **包括发出这次请求的那条**——于是「按下钮」的成功路径本身就常常表现为一次传输失败。
 * 把传输失败当失败，用户会看到「明明开了却报错」；把它当成功，则真的失败（比如端口被别的
 * 进程占了）也会被说成成功。
 *
 * 唯一能分开两者的读数是**改完之后的绑定地址**：故本模块一律以「重读 `/server-info` 的
 * 结果」为判定依据，服务端给的那条报文只在**它确实没变成目标状态**时用来解释原因。
 *
 * 判据住在这里（纯逻辑 + 注入依赖）而不是组件里，是为了能直接钉住：传输失败后的重读、
 * 重试窗口、以及「真的失败」与「没读回来」两种说法的分界。
 */

/** 本模块用到的三个出口（生产是 `api/client.ts` 的三个函数）。 */
export interface LanToggleDeps {
  setLan(enabled: boolean): Promise<unknown>;
  clearLan(): Promise<unknown>;
  info(): Promise<ServerInfo>;
  /** 重读之间的等待（测试注入即得到确定的时间线）。 */
  sleep(ms: number): Promise<void>;
}

/** 一颗钮按下去之后的结果。 */
export type LanToggleResult =
  | { ok: true; info: ServerInfo; note: string | null }
  | { ok: false; message: string };

/** 重读窗口：改绑期间有几毫秒没有监听者，故重读要带重试（决策 186）。 */
export const LAN_VERIFY_ATTEMPTS = 8;
export const LAN_VERIFY_INTERVAL_MS = 250;

/** 目标「要绑成回环吗」——与 `ServerInfo.loopback_only` 同一口径，判据与顶栏共用一份
 * （`lib/localPage.ts::isLoopbackHostname`，决策 190）：绑什么算回环只能有一个答案。 */
const isLoopbackHost = isLoopbackHostname;

/** 传输层失败（连接被切断）而不是服务端的拒绝：只有后者带着一句可读的报文。 */
function isTransportFailure(err: unknown): boolean {
  const message = err instanceof Error ? err.message : String(err);
  return /failed to fetch|networkerror|load failed|fetch failed|network request failed/i.test(
    message,
  );
}

function failureMessage(err: unknown): string | null {
  if (err == null) return null;
  const message = err instanceof Error ? err.message : String(err);
  return message.trim() === '' ? null : message;
}

/**
 * 按下「绑定全网卡」（`enabled = true`）或「只绑本机 / 恢复配置文件」（`false`），
 * 并以**重读到的绑定地址**为准给出结果。
 *
 * `enabled = false` 走的是清除接口（回到启动参数 / 配置文件那一级，决策 186），不是
 * 写死 `127.0.0.1`——两者在「启动参数指定了 0.0.0.0」时结果不同，而用户点这颗钮的意思是
 * 「别让界面说了算」。
 */
export async function changeLanMode(
  enabled: boolean,
  deps: LanToggleDeps,
): Promise<LanToggleResult> {
  let requestError: unknown = null;
  try {
    if (enabled) {
      await deps.setLan(true);
    } else {
      await deps.clearLan();
    }
  } catch (err) {
    requestError = err;
  }

  const target = !enabled; // 目标 loopback_only
  let last: ServerInfo | null = null;
  for (let attempt = 0; attempt < LAN_VERIFY_ATTEMPTS; attempt++) {
    try {
      last = await deps.info();
      if (isLoopbackHost(last.host) === target) {
        return {
          ok: true,
          info: last,
          note: successNote(enabled, last, requestError !== null),
        };
      }
    } catch {
      // 重读本身失败（改绑空窗）：继续重试，窗口走完还读不到才算失败
    }
    if (attempt < LAN_VERIFY_ATTEMPTS - 1) {
      await deps.sleep(LAN_VERIFY_INTERVAL_MS);
    }
  }

  // 没变成目标状态：服务端给了报文就用它（那是「真的失败」，如端口被占），
  // 否则只能说清「没读到目标状态」——不编造原因。
  const serverMessage =
    requestError !== null && !isTransportFailure(requestError)
      ? failureMessage(requestError)
      : null;
  return {
    ok: false,
    message: serverMessage ?? unreachedMessage(enabled, last),
  };
}

/**
 * 成功了也要说实话的两种情形（决策 186）：
 * - 这次请求的连接被改绑切断了（常态，不是错误）；
 * - 结果是**启动参数**定的，那界面上的选择重启后不生效——用户有权知道。
 */
function successNote(enabled: boolean, info: ServerInfo, transportFailed: boolean): string | null {
  if (enabled && info.bind_source === 'startup' && !transportFailed) {
    return '当前是启动参数（--host / AGENTPIPELINE_LAN）指定的对外绑定；界面上的选择已记住，但重启后仍以启动参数为准。';
  }
  if (transportFailed) {
    return '改绑已生效（这次请求的连接被改绑切断了，属正常）';
  }
  return null;
}

/** 没到目标状态时的说明：能归因就归因，不能就只说现象。 */
function unreachedMessage(enabled: boolean, last: ServerInfo | null): string {
  if (last === null) {
    return '改绑没有生效：读不到服务状态。可重试，或改用启动参数 --host 0.0.0.0。';
  }
  if (last.bind_source === 'startup') {
    return enabled
      ? `改绑没有生效：当前仍绑定 ${last.host}，且绑定由启动参数指定——请去掉 --host / AGENTPIPELINE_LAN 后重启，再从这里开。`
      : '关不掉：启动时的 --host / AGENTPIPELINE_LAN 指定了对外绑定，界面清掉选择也改不了这一次——请用不带该参数的方式启动。';
  }
  return `改绑没有生效：当前仍绑定 ${last.host}。可重试，或改用启动参数 --host 0.0.0.0。`;
}
