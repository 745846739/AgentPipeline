import { afterEach, describe, expect, it, vi } from 'vitest';
import { labelFor } from './toolLabels';

/**
 * 回执标签的取数与查表（决策 247⑤）。
 *
 * 判据打在**取数的那一跳**上（fetch 计数与失败后的重试），不是打在映射规则上——映射只有一行
 * `?? `，而「取一次就够」「失败不把标签永久退化成英文」这两件是会随重构丢掉的行为。
 * 模块级缓存跨用例存活，故每次用例都 `vi.resetModules()` 后重新 import。
 */

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

async function freshLoad() {
  vi.resetModules();
  const mod = await import('./toolLabels');
  return mod.loadToolLabels;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('labelFor', () => {
  it('登记在册的给后端那个词，认不出的原样显示', () => {
    const labels = { read_task: '读任务台账', write_file: '写文件' };
    expect(labelFor(labels, 'read_task')).toBe('读任务台账');
    // 清单外的工具名照旧显原名（决策 200 的平实口径；e2e 钉着 spawn_sub_agent）
    expect(labelFor(labels, 'spawn_sub_agent')).toBe('spawn_sub_agent');
    // 取数还没回来时（空表）同样原样——短暂的英文总好过错的中文
    expect(labelFor({}, 'read_task')).toBe('read_task');
  });
});

describe('loadToolLabels', () => {
  it('取数一次：先后两次调用只发一跳，映射按 name 建表', async () => {
    const fetchMock = vi.fn(async () =>
      jsonResponse({
        tools: [
          { name: 'read_task', label: '读任务台账' },
          { name: 'read_diagnosis', label: '读诊断包' },
        ],
      }),
    );
    vi.stubGlobal('fetch', fetchMock);
    const load = await freshLoad();

    const first = await load();
    const second = await load();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(first).toEqual({ read_task: '读任务台账', read_diagnosis: '读诊断包' });
    expect(second).toBe(first);
  });

  it('失败不缓存：这一跳失败，下一跳照常重试（标签不永久退回英文原名）', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => jsonResponse({ error: '还没起来' }, 500)));
    const load = await freshLoad();
    await expect(load()).rejects.toThrow();

    const fetchMock = vi.fn(async () =>
      jsonResponse({ tools: [{ name: 'run_readonly', label: '只读取证' }] }),
    );
    vi.stubGlobal('fetch', fetchMock);
    await expect(load()).resolves.toEqual({ run_readonly: '只读取证' });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});
