# 已归档原型（deprecated）

## 现行原型

**主题六「像素机房 · 夜班流水线」**四种，规格见 [../theme-6-pixel.md](../theme-6-pixel.md)
（2026-09-13 起为**现行视觉规格**，决策 169）：

- [../prototype-pixel.html](../prototype-pixel.html) —— 深色桌面
- [../prototype-pixel-light.html](../prototype-pixel-light.html) —— 浅色桌面「掌机背光」
- [../prototype-pixel-mobile.html](../prototype-pixel-mobile.html) —— 深色移动
- [../prototype-pixel-mobile-light.html](../prototype-pixel-mobile-light.html) —— 浅色移动

## 归档表

停更的原型移到本目录，仅作设计参照与决策记录——不再随交互骨架演进同步。

| 原型 | 原主题 | 规格 | 退役 |
|---|---|---|---|
| [prototype-terminal.html](prototype-terminal.html) | ③ 终端「调度电报」 | [theme-3-terminal.md](theme-3-terminal.md) | 2026-09-13（决策 169：视觉切换为主题六） |
| [prototype-terminal-light.html](prototype-terminal-light.html) | ③ 终端（浅色「绿杠电报纸」） | 同上 | 同上 |
| [prototype-terminal-mobile.html](prototype-terminal-mobile.html) | ③ 终端（移动） | 同上 | 同上 |
| [prototype-terminal-mobile-light.html](prototype-terminal-mobile-light.html) | ③ 终端（移动浅色） | 同上 | 同上 |
| [prototype.html](prototype.html) | ① 夜间调度台 | [../frontend-design.md](../frontend-design.md) —— 交互骨架与页面元素清单仍是全站准绳；其 §3 视觉语言属本款，已停更 | 2026-09-12（选型收敛至主题三） |
| [prototype-minimal.html](prototype-minimal.html) | ② 日间时刻表 | — | 2026-09-12 |
| [prototype-blueprint.html](prototype-blueprint.html) | ④ 蓝图「晒图房」 | [../theme-4-blueprint.md](../theme-4-blueprint.md)（已归档方向） | 2026-09-12 |
| [prototype-workshop.html](prototype-workshop.html) | ⑤ 车间「工单板」 | [../theme-5-workshop.md](../theme-5-workshop.md)（已归档方向） | 2026-09-12 |

## 用法约定

- 本目录文件**不再更新**：改动只进正在维护的原型，避免同一份交互骨架出现两个版本。
- 仍可引用：`prototype.html` 是交互骨架的第一个完整样本，`frontend-design.md` 的页面元素
  清单仍以它为基线；主题三的四份是「字符终端」这一视觉路线的完整记录。
- 新增原型不要以本目录为基线派生（骨架会漂移）；以现行四款或 `frontend-design.md` §4–§7 为准。
- 这些文件是自包含的（无本地相对链接），移动后可直接打开；`#hash` 直达仍有效。
  主题三的规格 `theme-3-terminal.md` 同在本目录，其内部互链保持在同目录内。
