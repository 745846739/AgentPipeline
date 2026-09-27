/**
 * 深浅配色的运行时状态（决策 169 的两套配色：夜班靛 / 掌机背光）。
 *
 * 为什么是 store 而不是组件里的 `$state`（决策 300）：切换钮有**两处挂载**——桌面档的
 * 底部状态行，与设置落地页页头（窄档状态条整条退场后，手机端只剩后者）。两份各自的状态
 * 会在同一屏上给出两个互相矛盾的可访问名：在落地页按过一次，底部那枚还写着旧的
 * 「切换到浅色主题」，读屏念出来的下一步是反的。
 *
 * 职责边界：`index.html` 的首屏脚本负责把本地记忆写进 `data-theme`（避免首屏闪色），
 * 这里只管**按钮要显示哪一个**、以及把切换结果落回同一个键——键名 `agentpipeline.theme`
 * 与两套配色的取值一处都不新增。
 */
const STORAGE_KEY = 'agentpipeline.theme';

function savedTheme(): 'dark' | 'light' {
  try {
    if (typeof localStorage === 'undefined') return 'dark';
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === 'light' || saved === 'dark') return saved;
  } catch {
    // 隐私模式等禁用 localStorage：本次会话用缺省深色（与 index.html 同一口径）
  }
  return 'dark';
}

class ThemeStore {
  /** 当前配色：按钮的文字与 `aria-label` 都由它推出（按钮写的是「按下去去哪儿」）。 */
  current = $state<'dark' | 'light'>(savedTheme());

  set(next: 'dark' | 'light'): void {
    this.current = next;
    if (typeof document !== 'undefined') document.documentElement.dataset.theme = next;
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // 隐私模式等禁用 localStorage：本次会话仍生效
    }
  }

  toggle(): void {
    this.set(this.current === 'dark' ? 'light' : 'dark');
  }

  /** 测试用：回到本地记忆里的值。 */
  reset(): void {
    this.current = savedTheme();
  }
}

export const theme = new ThemeStore();
