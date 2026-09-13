import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  ApiError,
  deleteStageConfig,
  getServerInfo,
  listStageConfigs,
  putStageConfig,
  qrSvgUrl,
} from './client';
import { CLIENT_HEADER, setApiBase } from './config';

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
