# 04: 钉路径——私有 shim 目录 + 子进程 PATH 前置

**What to build:** 启用时把探测解析到的绝对路径钉成 `{home}/rtk-shim/` 里**唯一**一个 `rtk` 符号链接；
运行期由 `CommandRunner` 把这个目录前置进**子进程**的 PATH（`cmd.env("PATH", …)`）。于是从 Finder 启动
的桌面壳（继承 launchd 的最小 PATH，`/usr/local/bin` 不在里面）也能真的用到 rtk——
**「启用时说可用」与「真跑起来可用」成为同一件事**，否则最坏的形状是设置页说可用、命令全 127。
后端同时支持「手填路径」作为解析的最后兜底。

**Blocked by:** 03

**Status:** done（已实现；「手填路径」的语义按票内判据订正，见下）

- [x] 最小 PATH（`/usr/bin:/bin:/usr/sbin:/sbin`，模拟 Finder 启动）下启用后命令仍走 rtk——这是本票
      存在的全部理由，必须有测试
      → 两条：`process.rs::the_shim_is_prepended_to_any_base_path_including_the_minimal_one`（纯合成
      规则，含空 `PATH`）、`command_funnel.rs::the_shim_is_what_resolves_rtk_for_the_child`（**真的
      起一个子进程**，`command -v rtk` 指向 shim 里那一份）。
- [x] shim 目录只含 `rtk` 一个名字，前置它**不改别的命令的解析**（有测试：`python3` 之类仍解析到原处）。
      这是它与「把 `/usr/local/bin` 前置」的区别
      → `prepending_the_shim_does_not_change_other_resolutions` + `shim_holds_exactly_one_name_and_leaves_no_residue`。
- [x] 关掉开关 / 改路径 → shim 跟着变，不留残迹（符号链接指向失效时不静默假装可用）
      → **评审收口②**：首版 `unpin` 写了没人调，「不留残迹」落空；现在关掉开关就拆目录（接线用例
      断言 `!rtk::shim_dir(home).exists()`），`pin` 换目标时重建、同目标幂等（收口③：首版每次重建
      会开一个「shim 里暂时没有 `rtk`」的空窗）。
- [ ] **不跑用户的登录 shell**（不执行 `$SHELL -lc 'command -v rtk'`），有断言钉住这条纪律；
      解析顺序只有服务进程 PATH → 已知目录（`/usr/local/bin` / `/opt/homebrew/bin` / `/opt/local/bin` /
      `~/.local/bin` / `~/.cargo/bin`）→ 手填路径
      → **半勾，故整条不勾**：前半成立且**评审收口⑤**补上了断言（`the_users_login_shell_is_never_run`
      放一个会写标记的 `$SHELL`，标记出现即失败）。后半**按本票第 5 条判据改写了**：手填路径落地为
      **优先**，不是最后一根稻草——见下一条。
- [x] 手填路径能覆盖自动解析；填错时失败可见可归因（不静默回落到自动解析）
      → 这一条与上一条的「解析顺序」表述在票内本就冲突（「覆盖」与「排在最后」不可兼得），落地取
      本条的语义：**填了就用它**（`Source::Manual`），没填才走 服务进程 PATH → 五个已知目录。填错
      时三种错各说一句（空 / 不是绝对路径 / 那儿没有可执行的 rtk），**不回落**——测试
      `manual_path_overrides_and_never_falls_back`。显式偏离 spec §4 的「兜底」措辞，spec 已就地
      订正，决策 297 正文记的也是订正后的语义。
- [x] 四门 + 交付说明

## Comments

- **为什么必须钉**（spec §4）：桌面壳由 Finder 直接 exec（`crates/desktop/src/main.rs:44`），继承 launchd
  的最小 PATH（本机 `launchctl getenv PATH` 为空 → `/usr/bin:/bin:/usr/sbin:/sbin`），`/usr/local/bin/rtk`
  不在里面；而命令行起 `agent-pipeline serve` 则继承 shell 的 PATH。所以「本机装了 rtk」与「这个服务能用
  rtk」是两个答案，后者才是唯一有意义的那个。
- **为什么是私有 shim 而不是把 `/usr/local/bin` 前置**：那个目录里还有一堆别的二进制，前置它会顺手改掉
  别的命令的解析（`python3` 之类）。shim 只影响 `rtk` 一个名字。
- **为什么必须靠 PATH 前置、而不是把绝对路径插进命令串**：改写器吐出来的是裸 `rtk`，靠 PATH 找；对每段
  做字符串手术（把 `rtk ` 换成绝对路径）既脆，还要在有引号的地方做手术。
- **为什么不在服务进程里执行用户的 rc 文件**：那是静默副作用 + 数百毫秒延迟；决策 185 之后本仓对
  「偷偷扫 PATH」一贯的处置是删掉。失败可见、可修就够了。
