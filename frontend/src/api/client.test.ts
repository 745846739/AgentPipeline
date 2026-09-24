import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  ApiError,
  deleteStageConfig,
  getForemanSession,
  getServerInfo,
  KIND_REQUEST_TIMEOUT,
  listStageConfigs,
  mapRequestError,
  putStageConfig,
  qrSvgUrl,
} from './client';
import { CLIENT_HEADER, PAIRING_HEADER, clearPairingToken, setApiBase, setPairingToken } from './config';

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('stage_configs 客户端（票 22）', () => {
  it('PUT 的 4xx `{ error }` 原样抛成 ApiError.message', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => jsonResponse({ error: '引用的 provider 不存在或已禁用：p9' }, 400)),
    );
    const err = await putStageConfig('develop', { provider_id: 'p9' }).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.status).toBe(400);
    expect(err.message).toBe('引用的 provider 不存在或已禁用：p9');
  });

  it('DELETE 的 400（如 cross_family_judge 依赖）也回显后端原因', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => jsonResponse({ error: '删除 validator_cross_check 会导致启动校验失败' }, 400)),
    );
    const err = await deleteStageConfig('validator_cross_check').catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.status).toBe(400);
    expect(err.message).toContain('validator_cross_check');
  });

  // 决策 154 的后续票：拼错的工具名从「静默丢弃」改为后端**拒绝写入**。
  // 那句报文要一路走到表单上（`SettingsStages` 把捕获到的 message 交给
  // `StageConfigForm` 的 error 槽）——不吞错、也不因为保存按钮按过就当成功。
  it('未知工具名的 400 报文原样回显（含未知名字与已知集合）', async () => {
    const backend =
      '阶段 develop 的 tools_json 声明了 v1 不存在的工具：web_search（v1 已知工具集：write_file / ... / spawn_sub_agent）';
    vi.stubGlobal('fetch', vi.fn(async () => jsonResponse({ error: backend }, 400)));
    const err = await putStageConfig('develop', { tools_json: ['web_search'] }).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.status).toBe(400);
    expect(err.message).toContain('web_search');
    expect(err.message).toContain('v1 已知工具集');
  });

  it('写请求恒带 X-AgentPipeline 头（决策 128 / 153③），GET 不带', async () => {
    const fetchMock = vi.fn(async () => jsonResponse({ ok: true, stage_configs: [] }));
    vi.stubGlobal('fetch', fetchMock);

    await putStageConfig('review', { max_tokens: 2048 });
    await deleteStageConfig('review');
    await listStageConfigs();

    const calls = fetchMock.mock.calls as unknown as Array<[string, RequestInit]>;
    expect(calls).toHaveLength(3);
    const putHeaders = calls[0][1].headers as Record<string, string>;
    const delHeaders = calls[1][1].headers as Record<string, string>;
    const getHeaders = calls[2][1].headers as Record<string, string>;
    expect(putHeaders['X-AgentPipeline']).toBe('1');
    expect(delHeaders['X-AgentPipeline']).toBe('1');
    expect(getHeaders['X-AgentPipeline']).toBeUndefined();
    expect(calls[0][0]).toBe('/stage-configs/review');
    expect(calls[0][1].method).toBe('PUT');
  });
});

describe('server-info 客户端（决策 167）', () => {
  it('getServerInfo 走同源相对路径且不带写头（纯 GET）', async () => {
    const fetchMock = vi.fn(async () =>
      jsonResponse({
        host: '0.0.0.0',
        port: 8787,
        loopback_only: false,
        port_source: 'config',
        addresses: [{ interface: 'en0', url: 'http://192.168.1.10:8787', preferred: true }],
      }),
    );
    vi.stubGlobal('fetch', fetchMock);

    const info = await getServerInfo();

    const calls = fetchMock.mock.calls as unknown as Array<[string, RequestInit]>;
    expect(calls[0][0]).toBe('/server-info');
    const headers = calls[0][1].headers as Record<string, string>;
    expect(headers[CLIENT_HEADER]).toBeUndefined();
    expect(info.addresses[0].preferred).toBe(true);
    expect(info.loopback_only).toBe(false);
  });

  it('qrSvgUrl 把地址正确转义进 query（含冒号与斜杠）', () => {
    setApiBase(null);
    expect(qrSvgUrl('http://192.168.1.10:8787')).toBe(
      '/server-info/qr.svg?url=http%3A%2F%2F192.168.1.10%3A8787',
    );
  });

  it('qrSvgUrl 跟随注入的 api base（桌面壳形态）', () => {
    setApiBase('http://127.0.0.1:9100');
    expect(qrSvgUrl('http://127.0.0.1:9100')).toBe(
      'http://127.0.0.1:9100/server-info/qr.svg?url=http%3A%2F%2F127.0.0.1%3A9100',
    );
    setApiBase(null);
  });
});

describe('配对令牌压在请求头上（决策 182㉙，票 07）', () => {
  it('已配对时所有方法都带头——含只读 GET（对讲台读接口在局域网形态同样受护）', async () => {
    const calls: Array<{ url: string; init?: RequestInit }> = [];
    vi.stubGlobal(
      'fetch',
      vi.fn(async (url: string, init?: RequestInit) => {
        calls.push({ url: String(url), init });
        return jsonResponse({ messages: [], total_tokens: 0, total_calls: 0 });
      }),
    );
    setPairingToken('tok-42');
    await getForemanSession();

    const headers = calls[0].init?.headers as Record<string, string>;
    expect(headers[PAIRING_HEADER]).toBe('tok-42');
    // GET 不带旁路头（决策 128 只拦写请求），但配对头必须在
    expect(headers[CLIENT_HEADER]).toBeUndefined();
    clearPairingToken();
  });

  it('未配对时不带这个头——回环形态零摩擦', async () => {
    const calls: Array<{ init?: RequestInit }> = [];
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_url: string, init?: RequestInit) => {
        calls.push({ init });
        return jsonResponse({ messages: [], total_tokens: 0, total_calls: 0 });
      }),
    );
    clearPairingToken();
    await getForemanSession();
    const headers = calls[0].init?.headers as Record<string, string>;
    expect(PAIRING_HEADER in headers).toBe(false);
  });
});

describe('请求失败的口径（票 12 / R2-14）', () => {
  it('超时：说清等了多久（`AbortSignal.timeout` 抛的是 TimeoutError）', () => {
    const err = new Error('signal timed out');
    err.name = 'TimeoutError';

    const mapped = mapRequestError(err, 30_000, false);

    expect(mapped).toBeInstanceOf(ApiError);
    expect(mapped.status).toBe(0);
    expect(mapped.message).toContain('超时');
    expect(mapped.message).toContain('30 秒');
    // 超时带机器可读 kind（票 06）：界面分支只认它，报文是给用户看的
    expect(mapped.kind).toBe(KIND_REQUEST_TIMEOUT);
  });

  it('没有调用方 signal 的 AbortError 也算超时', () => {
    const err = new Error('aborted');
    err.name = 'AbortError';
    expect(mapRequestError(err, 5_000, false).message).toContain('超时');
  });

  it('有调用方 signal 的 AbortError 是**主动取消**，不冒充超时', () => {
    const err = new Error('aborted');
    err.name = 'AbortError';

    const mapped = mapRequestError(err, 5_000, true);

    expect(mapped.message).not.toContain('超时');
    expect(mapped.message).toContain('网络请求失败');
  });

  it('网络本身不通：保留内核给的原因', () => {
    const mapped = mapRequestError(new TypeError('Failed to fetch'), 30_000, false);
    expect(mapped.message).toBe('网络请求失败：Failed to fetch');
    // 不是超时就不带超时的 kind（票 06：附不附「仍在继续」那句全看它）
    expect(mapped.kind).toBeUndefined();
  });

  it('拿不到 message 也不许抛：退化成一条可读的话', () => {
    expect(mapRequestError('boom', 1_000, false).message).toBe('网络请求失败：boom');
  });

  it('每个请求都带上了超时 signal（不是只有调用方显式传的那些）', async () => {
    const calls: Array<RequestInit | undefined> = [];
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_url: string, init?: RequestInit) => {
        calls.push(init);
        return jsonResponse({ ok: true, stage_configs: [] });
      }),
    );

    await listStageConfigs();

    expect(calls[0]?.signal).toBeInstanceOf(AbortSignal);
  });
});
