# 09: e2e 外发（第二期）——白名单扩展 + 产物走 GitHub artifact

**What to build:** 决策 382① 的落地：把 e2e 走查（playwright + 真服务 + 真浏览器）
纳入 offload 外发范围。两件事：

1. **白名单扩展**（决策 381③ 的纪律照走）：`offload_command_allowed` 增加两个
   前缀——`UX_AUDIT=1 … playwright test`（环境变量赋值形态的前缀匹配要单独设计，
   现有匹配是「整串以 prefix 开头」）与 `bash scripts/e2e-artifacts.sh`；各自过一遍
   防注入面审查（组合符/命令替换/重定向照旧全拒）。
2. **产物通道**：offload.yml 为 e2e 命令上传 GitHub artifact（截图/报告目录），
   工具回执只回文本摘要 + artifact 名/URL——「产物不回传（106→GitHub ~197KB/s）」
   的共识不变，要看图的人用 `gh run download` 或网页。

**Blocked by:** 06（已落地;真实链路验收已过）

**Status:** ready-for-agent

- [ ] 白名单扩展 + 正反例测试（含环境变量前缀形态）
- [ ] offload.yml 按 command 形态分流上传 artifact（cargo 命令不传）
- [ ] 工具回执带 artifact 名与 URL;轮询/对账/回退语义不动（决策 381⑤）
- [ ] 真实链路验收:在 106 用一个自然任务跑一轮 e2e 外发（走查截图可从
      GitHub artifact 取回）
