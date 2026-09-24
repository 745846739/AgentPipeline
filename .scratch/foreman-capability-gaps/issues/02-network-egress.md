# 02: 网络出口——给值班长一个受治理的只读网口

**What to build:**（先裁决、后动手——治理形状见文末）值班长深挖时经常需要外部世界：
报错搜社区、版本查文档、依赖查 CVE。现状它**没有专用网络工具**，而讽刺的是
`run_command` 在 `auto` 档下**事实上已经能上网**（`curl` 没被挡）——即「开不开网」
不是这个问题，问题是**网现在是开着的、却没有任何治理**（无审计口径、无超时档、
无外发面白名单，且随档位整个消失或整个敞开）。

**Blocked by:** None（拷问即可开始；裁决落 `docs/decisions.md` 后实现）

**Status:** done（2026-09-24）

**撞/踩的裁决（决策 262②）：** 决策 206 的域是**文件路径域**（`home.root()` + `data/` 前缀拒），
网络从来不在它的裁决面里——网口是新面；决策 112 说库里**明文存 provider 密钥**，
任何「把内容发出去」的工具都碰外发面（diagnosis 包里有 prompt 原文与日志行，不能原样出境）。

**拷问时要裁的分支：**

- **(a) 新只读工具 `web_fetch` / `web_search`**（推荐起点）：像 `run_readonly`（决策 237）那样
  治理——白名单语义（GET-only？域名策略？）、独立超时档、**落命令台账**（session 归属，
  `GET /foreman/commands` 现成）、走 `NetworkPolicy::egress` 既有出口策略（`tools.rs` 已有
  构造期取用的 `egress` 句柄，`run_command` 的回环放行就是它管的）。
- **(b) 把 `curl`/`wget` 加进 `run_readonly` 白名单**：改动最小，但 `run_readonly` 的承诺是
  「改不了任何东西」——`curl -X POST` 能改；且 argv 直出挡不住「把密钥贴进 URL」。
- **(c) 不给网，网永远随 `run_command` + 档位走**：承认现状，把外发治理并进 179 的
  egress 策略细化——省一个工具，但深挖场景要么等人开 `auto`、要么没有审计粒度。

**边界草案（拷问输入，不是结论）**：请求体出境前过一道**脱敏面**（`data/` 读不到已是硬规则，
但 diagnosis 内容进 prompt 后再出境是另一条缝）；值守轮该不该用网（夜班自主搜报错 vs
夜间外发无人盯）单独裁；工具归 A/B 层哪一层、受不受 `env_mode` 管（它只读，但出的是网）。

- [x] 三选一裁决取 **(a) 新只读工具 `web_fetch`**（用户四问，决策 266；`web_search` 不单列——
      搜索 = `web_fetch` 指向放行的搜索端点、正文由模型自读，理由见 266①）；
      白名单形状 = `NetworkPolicy::allows` 同一张（决策 179，零新配置面）、超时 = 15s 缺省 +
      `timeout_sec`（1–120）、脱敏面 = `sanitize_command_line` 过台账行 + spec 描述禁密钥入 URL、
      台账字段 = `web_fetch <url>` 起止两段（成功 0 / 出口拒 `EGRESS_DENIED_EXIT_CODE` /
      校验拒退出码留空）
- [x] 「值守轮能否用网」单独裁决：**不能（首期）**（入 `FOREMAN_WATCH_TOOL_DENY`）
- [x] 实现 + 用例落齐（决策 266）：L2 新文件 `tests/integration/web_fetch.rs` **6 条**
      （回环取回 + 会话维度台账 / 未放行发请求前 PolicyDenied / 裸 http 判拒 / 显式超时 /
      非文本拒收 / 档位不管而值守 deny 摘除），冻结契约 21→23、watch 禁单扩 4
