# 04: O4-B 续接简报化——重置的轮带「简报+产物路径」起跑,不带全卷转录

**来源:** 同 01 的监控实录。续接两轮失败后「空白重跑一次」:每轮重置都重新侦察
(git status/log、freshness 检查、ux-audit-3 是否存在反复确认、重跑 playwright),
90 分钟耗尽时还没走到产出。侦察结果本该是**落盘的文件**,却只活在转录里,
每次重置都要重新花钱买一遍。

**Blocked by:** 03(压缩硬底先行:先有体量兜底,简报化才有干净的地基——否则
简报与全卷并存的判据没法定) → 03 已 done

**Status:** done(2026-10-02,决策 379)

- [x] 「空白重跑」(timeout 梯子第 3 档)的 prompt 改为:任务描述 + 阶段产出
      文件清单(`.scratch/<feature>/` 现状)+ 最近一条收口摘要,**不带**全卷转录
      —— 新模块 `pipeline/continuation_brief.rs`;`.scratch/` 现状列入工作区一节的
      目录清单,收口摘要取**最后一条非 running** 的 run
- [x] 续接(第 1–2 档)保持转录续接不变(决策 320 的口径);仅第 3 档换形态
      —— `ContinuationMode::{Transcript, Brief}`,`take_continuation` 按原因分流
- [x] 简报里显式列出 worktree 中已有的未提交改动与未落盘目标
      (本轮事故的直接教训:改过的 `ux-audit.spec.ts` 差点随 worktree 蒸发)
      —— `Git::dirty_files`(逐条 `XY path`,先排序再截断)+ `ProductTarget` 目标表
- [x] 集成测试:造两轮超时 → 第 3 轮 prompt 断言不含全卷转录、含简报要素
      —— `the_blank_restart_round_starts_from_a_brief_not_the_transcript`(注意:
      连续超时 1–2 次**续接**、第 3 次才降级,故简报那一轮是**执行第 4 轮**)

**边界.** 不动梯子档位与阈值(`TIMEOUT_AUTO_CONTINUES_MAX=2`,决策 368 刚钉过);
不改 resume(人按继续)路径——人按的继续仍带全卷转录。

**验收实录（2026-10-02）.** 四轮真执行体跑满梯子(每轮先干一次工具轮再停在第二次
调用上被看门狗判超时),第 4 轮首个请求 `messages` 为空、user prompt 含四节齐备
(任务描述 / 阶段产物文件清单含工作区 `.scratch/` 现状 / 未提交改动清单点得出
`STALE_FIX.txt` / 最近收口摘要取 attempt 3 而非起跑中的 attempt 4),对照断言第 1–2 档
仍是带 `messages` 的全卷续接。**集成测试抓到的两处真问题已修**:收口摘要原先取
「最后一条 run」会读到起跑这一轮自己的 `running` 行;`dirty_files` 原先先截断再排序,
超上限时丢掉的可能恰是最该保护的那个文件。
