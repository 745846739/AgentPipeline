# 02: 运维——106 上登录 gh,替换失效 token

**What to build:** 106(106.12.12.6,root)上的 gh 凭据已失效(`gh auth status` 报
`/root/.config/gh/hosts.yml` 的 token invalid)。以设备码流程在 106 上重新
`gh auth login`(与本地机同构),恢复两条既有能力:kanban 分支的 HTTPS 推送路径,
以及后续外发工作流的触发与结果读取(`gh workflow run` / `gh run view`)。
纯运维动作,不涉及代码改动。

**Blocked by:** None(可立即开始;建议尽早,当前推送路径可能已在用死 token)

**Status:** **done**(2026-10-03,设备码流程授权;证据见下)

- [x] 106 上 `gh auth status` 显示 logged in 且 token 有效
      —— `✓ Logged in to github.com account 745846739`,Token scopes: `read:org, repo, workflow`
      (repo 给推送、workflow 给 workflow_dispatch,恰好覆盖票 06 触发外发所需)
- [x] 推送路径恢复:`credential.https://github.com.helper = !/usr/bin/gh auth git-credential`
      (`gh auth setup-git` 落好),`git push --dry-run origin HEAD:refs/heads/gh-auth-check`
      成功到达远端(未建引用)
- [x] `gh workflow list` 看到本仓 workflows:`check` / `deploy-106` / `offload` / `zz-debug`,
      其中 **offload 已 active**(票 04 的工作流已在默认分支上,票 06 的触发目标就位)

> **授权方式记录**:设备码流程(client_id 用 gh 官方公共值),脚本在 106 上轮询取 token 后
> 直接经 `gh auth login --with-token` 落盘——token 不出现在任何日志/对话里。
> 授权人:745846739,2026-10-03。
