import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import type { SkillSummary } from '../../api/types';
import StageConfigForm from './StageConfigForm.svelte';

/**
 * `tools_json` 的自由文本与后端准入（决策 154 的后续票）。
 *
 * 票面第 7 条要的是「前端在收到 400 时呈现后端错误信息（不吞错、不伪造成功）」，
 * 而那条链路只有两段：**端点把 `{ error }` 抛成 `ApiError.message`**（在
 * `api/client.test.ts` 里钉住）与**表单把这句原文渲染出来**（本文件钉住）。
 * 中间不经过任何改写——故这条用例喂给表单的 error 串就是后端 400 报文本身。
 */

const SKILLS: SkillSummary[] = [];

describe('阶段配置表单的错误呈现', () => {
  it('把后端 400 报文原样显示（含未知工具名与已知集合）', () => {
    const message =
      '阶段 develop 的 tools_json 声明了 v1 不存在的工具：web_search（v1 已知工具集：write_file / edit_file / read_file / delete_file / list_dir / run_command / submit_metadata / Skill / spawn_sub_agent）';
    render(StageConfigForm, {
      props: {
        config: null,
        providers: [],
        skills: SKILLS,
        submitting: false,
        error: message,
        onsubmit: () => {},
        oncancel: () => {},
      },
    });

    // 不吞错：整句都在（不是「保存失败」这类被抹平的提示——照它才知道该写什么）
    expect(screen.getByText(message)).toBeTruthy();
  });

  it('提交时把草稿交给调用点（键名不私自改建，后端才知道该拒什么）', async () => {
    const onsubmit = vi.fn();
    render(StageConfigForm, {
      props: {
        config: null,
        providers: [],
        skills: SKILLS,
        submitting: false,
        error: null,
        onsubmit,
        oncancel: () => {},
      },
    });

    const tools = screen.getByLabelText('tools_json');
    await fireEvent.input(tools, { target: { value: '["web_search"]' } });
    await fireEvent.click(screen.getByRole('button', { name: '创建' }));

    expect(onsubmit).toHaveBeenCalledTimes(1);
    expect(onsubmit.mock.calls[0][0].tools_json).toBe('["web_search"]');
  });
});
