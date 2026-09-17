# 15: 环境变量值脱敏

**What to build:** 现有脱敏覆盖 URL 内嵌凭据与 `--token/--api-key/--password/--secret` 形式的参数值，以及若干密钥形态正则；但 `FOO=secret`、`export FOO=secret`、`${TOKEN}` 展开这类环境变量值未处理（决策 118 / spec §12.4.4 要求环境变量值替换为 `***`）。补上该模式，且保证脱敏仍在回填前生效——命令输出、命令记录、系统命令与工具结果四条路径统一。

**Blocked by:** None (can start immediately)

**Status:** done

- [x] `KEY=value` / `export KEY=value` / 变量展开形式的环境变量值被替换为 `***`
- [x] 无害变量不被误伤（如 `PATH=`、纯数字、已含 `***` 的值需有明确取舍并留用例）
- [x] 四条路径（工具命令输出、命令记录、系统命令、工具结果）脱敏行为一致
- [x] 脱敏在回填前生效，不出现「先落库后脱敏」的窗口
- [x] `docs/testing.md` §11 的「脱敏的环境变量值模式」条目更新为已关闭
