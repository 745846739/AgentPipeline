# 已归档原型（deprecated）

2026-09-12 选型收敛：前端视觉方向定为**主题三「终端 · 调度电报」**，其余四款停更。
停更的原型移到本目录，仅作设计参照与决策记录——不再随交互骨架演进同步。

| 原型 | 原主题 | 规格 |
|---|---|---|
| [prototype.html](prototype.html) | ① 夜间调度台 | [../frontend-design.md](../frontend-design.md) —— 交互骨架与页面元素清单仍是全站准绳；其 §3 视觉语言属本款，已停更 |
| [prototype-minimal.html](prototype-minimal.html) | ② 日间时刻表 | — |
| [prototype-blueprint.html](prototype-blueprint.html) | ④ 蓝图「晒图房」 | [../theme-4-blueprint.md](../theme-4-blueprint.md)（已归档方向） |
| [prototype-workshop.html](prototype-workshop.html) | ⑤ 车间「工单板」 | [../theme-5-workshop.md](../theme-5-workshop.md)（已归档方向） |

## 现行原型

主题三「终端 · 调度电报」四种，规格见 [../theme-3-terminal.md](../theme-3-terminal.md)：

- [../prototype-terminal.html](../prototype-terminal.html) —— 深色桌面（`#v-board` / `#v-run` / `#v-approve`）
- [../prototype-terminal-light.html](../prototype-terminal-light.html) —— 浅色桌面「绿杠电报纸」
- [../prototype-terminal-mobile.html](../prototype-terminal-mobile.html) —— 深色移动（`#v-board` / `#v-wait` / `#v-run` / `#v-approve`）
- [../prototype-terminal-mobile-light.html](../prototype-terminal-mobile-light.html) —— 浅色移动

## 用法约定

- 本目录文件**不再更新**：改动只进正在维护的原型，避免同一份交互骨架出现两个版本。
- 仍可引用：`prototype.html` 是交互骨架的第一个完整样本，主题三 §1 仍以它为页面元素基线。
- 新增原型不要以本目录为基线派生（骨架会漂移）；以现行四款或 `frontend-design.md` §4–§7 为准。
- 这些文件是自包含的（无本地相对链接），移动后可直接打开；`#hash` 直达仍有效。
