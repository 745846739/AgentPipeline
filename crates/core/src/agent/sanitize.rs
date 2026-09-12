//! 脱敏（决策 118，§12.4.4）。
//!
//! 时机是硬要求（决策 118）：输出脱敏在结果**回填 agent messages 之前**执行——
//! agent 看到的工具结果即脱敏后文本，落库与 context 同源。四条路径共用本模块：
//! 工具命令输出、命令记录（`kanban_node_commands.command`）、系统命令、工具结果。
//!
//! **脱敏契约（含取舍，代码本身看不出的约束）：**
//! 1. 密钥形态（`sk-*` / `ghp_*` / 长 base64）、URL 内嵌凭据、`--token/--api-key/
//!    --password/--secret` 取值一律打码；合法长 base64 被误伤是已知代价（决策 118）。
//! 2. 环境变量按**变量名**判定，而非按值：仅当变量名含 `TOKEN / SECRET / KEY /
//!    PASSWORD / PASSWD / CREDENTIAL / AUTH`（大小写不敏感，子串匹配）时，才把
//!    `NAME=value`、`export NAME=value`、`${NAME}` / `$NAME` 展开打码。因此
//!    `FOO=secret` 这类无害变量名**不会**被打码，即便值恰好叫 secret——这是为避免
//!    过度打码（`PATH=`、纯数字、日志字段）而显式接受的假阴性；`GIT_AUTHOR_*` /
//!    `*KEYBOARD*` 等含 `AUTH` / `KEY` 的无害名会被误伤，是同一取舍的另一面。
//! 3. 值已是 `***` 时再打一次结果不变（幂等），无需特判。
//! 4. `set -x` 的 `+ NAME=value` 形式可覆盖；但 shell 展开后的回显（`echo $TOKEN`
//!    打印成 `echo abc`）在脱敏层无从还原——这是已知边界，不在此处兜底。

use std::sync::OnceLock;

use regex::Regex;

const MASK: &str = "***";

/// 变量名里出现即视为敏感的关键词（大小写不敏感，子串匹配；契约见模块文档）。
const SECRET_NAME_HINTS: &[&str] = &[
    "TOKEN",
    "SECRET",
    "KEY",
    "PASSWORD",
    "PASSWD",
    "CREDENTIAL",
    "AUTH",
];

/// 变量名是否含敏感标记。
fn name_looks_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_NAME_HINTS.iter().any(|hint| upper.contains(hint))
}

fn key_regexes() -> &'static [Regex] {
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    RES.get_or_init(|| {
        vec![
            // OpenAI / DeepSeek 风格
            Regex::new(r"sk-[A-Za-z0-9_\-]{8,}").unwrap(),
            // GitHub PAT
            Regex::new(r"ghp_[A-Za-z0-9]{16,}").unwrap(),
            // 长 base64（已知会误伤，决策 118 显式接受）
            Regex::new(r"[A-Za-z0-9+/]{40,}={0,2}").unwrap(),
        ]
    })
}

/// 环境变量赋值 / 展开的正则（契约与取舍见模块文档；按变量名判定）。
fn env_regexes() -> &'static [Regex] {
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    RES.get_or_init(|| {
        vec![
            // `NAME=value` / `export NAME=value`；值可为单双引号串或非空白串。
            // 值首字符排除 `=`，避免把代码里的 `token == y` 误当成赋值。
            // 变量名交给 name_looks_secret 判定，非敏感名原样返回。
            Regex::new(
                r#"(?P<prefix>\b(?:export\s+)?(?P<name>[A-Za-z_][A-Za-z0-9_]*)=)(?P<val>"(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'|[^\s=]\S*)"#,
            )
            .unwrap(),
            // `${NAME}` / `$NAME` 展开（保留 $ / ${} 形态便于审计）。
            Regex::new(r"(?P<open>\$\{|\$)(?P<name>[A-Za-z_][A-Za-z0-9_]*)(?P<close>\})?")
                .unwrap(),
        ]
    })
}

/// 值本身是否「无害」：纯数字或已是 `***`。
///
/// 取舍（模块文档已述）：纯数字值不打码，避免 `MAX_TOKENS=4096` / `AUTH_TIMEOUT=30`
/// 这类数值配置被误伤；代价是全数字的弱口令（如 `PASSWORD=123456`）也不会打码。
fn value_is_harmless(val: &str) -> bool {
    val == MASK || val.chars().all(|c| c.is_ascii_digit())
}

fn credential_regexes() -> &'static [Regex] {
    static RES: OnceLock<Vec<Regex>> = OnceLock::new();
    RES.get_or_init(|| {
        vec![
            // URL 中的 user:password@
            Regex::new(r"(?P<scheme>https?://)(?P<user>[^/\s:@]+):(?P<pw>[^/\s@]+)@").unwrap(),
            // --token / --api-key / --password / --secret 的取值
            Regex::new(
                r"(?P<flag>--(?:token|api[-_]key|password|secret))(?P<sep>=|\s+)(?P<val>\S+)",
            )
            .unwrap(),
        ]
    })
}

/// 文本脱敏：密钥形态、URL / flag 凭据、敏感环境变量值一律替换为 `***`。
pub fn sanitize_text(text: &str) -> String {
    let mut out = text.to_string();
    // 环境变量先过：`TOKEN=sk-xxx` 若先走 key 正则会留下 `TOKEN=***`，
    // 结果虽同，但放在前面让「按名判定」的意图对 `TOKEN=plainvalue` 也成立。
    for re in env_regexes() {
        out = re
            .replace_all(&out, |caps: &regex::Captures| {
                let name = caps.name("name").map(|m| m.as_str()).unwrap_or_default();
                if !name_looks_secret(name) {
                    return caps[0].to_string();
                }
                if let Some(prefix) = caps.name("prefix") {
                    // NAME=value / export NAME=value
                    let val = caps.name("val").map(|m| m.as_str()).unwrap_or_default();
                    if value_is_harmless(val) {
                        caps[0].to_string()
                    } else {
                        format!("{}{MASK}", prefix.as_str())
                    }
                } else {
                    // ${NAME} / $NAME 展开：保留 $ / ${} 形态
                    let open = caps.name("open").map(|m| m.as_str()).unwrap_or("$");
                    let close = caps.name("close").map(|m| m.as_str()).unwrap_or("");
                    format!("{open}{MASK}{close}")
                }
            })
            .to_string();
    }
    for re in credential_regexes() {
        out = re
            .replace_all(&out, |caps: &regex::Captures| {
                if let Some(scheme) = caps.name("scheme") {
                    format!("{}{MASK}:{MASK}@", scheme.as_str())
                } else {
                    // flag + sep + mask（保留分隔符形态，便于审计看懂命令行）
                    let flag = caps.name("flag").map(|m| m.as_str()).unwrap_or_default();
                    let sep = caps.name("sep").map(|m| m.as_str()).unwrap_or(" ");
                    format!("{flag}{sep}{MASK}")
                }
            })
            .to_string();
    }
    for re in key_regexes() {
        out = re.replace_all(&out, MASK).to_string();
    }
    out
}

/// 命令脱敏：argv 拼接成命令行后脱敏（§12.4.4 记录的 `command` 列）。
pub fn sanitize_command(argv: &[String]) -> String {
    let joined = argv
        .iter()
        .map(|a| {
            if a.contains(char::is_whitespace) {
                format!("\"{a}\"")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    sanitize_text(&joined)
}

/// 单条命令字符串脱敏。
pub fn sanitize_command_line(command: &str) -> String {
    sanitize_text(command)
}

/// 命令 / 输出的统一脱敏入口（`run_recorded_command` 调用）。
pub fn sanitize_input(text: &str) -> String {
    sanitize_text(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_sk_and_ghp_tokens() {
        let out = sanitize_text("使用 key sk-abcdefghijklmnop12345678 调用");
        assert!(!out.contains("sk-abcdefghijklmnop12345678"));
        assert!(out.contains(MASK));

        let out = sanitize_text("token=ghp_ABCDEFGHIJKLMNOPQRSTUVWX");
        assert!(!out.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWX"));
    }

    #[test]
    fn masks_long_base64() {
        let secret = "QUJDREVGR0hJSktMTU5PUFFSU1RVVldYWVo0NTY3ODkwMTIzNDU2Nzg5MA==";
        let out = sanitize_text(&format!("blob: {secret}"));
        assert!(!out.contains(secret));
        assert!(out.contains(MASK));
    }

    #[test]
    fn does_not_mask_short_strings() {
        let out = sanitize_text("普通文本 http://127.0.0.1:8787/tasks 保留");
        assert!(out.contains("http://127.0.0.1:8787/tasks"));
    }

    #[test]
    fn masks_url_credentials() {
        let out = sanitize_text("clone https://alice:s3cret@github.com/a/b.git");
        assert!(!out.contains("s3cret"));
        assert!(!out.contains("alice:s3cret"));
        assert!(out.contains("https://***:***@github.com/a/b.git"));
    }

    #[test]
    fn masks_token_flag_argument() {
        let out =
            sanitize_command_line("curl -H x --token=ghp_ABCDEFGHIJKLMNOPQRSTUVWX https://api");
        assert!(!out.contains("ghp_ABCDEFGHIJKLMNOPQRSTUVWX"));
        assert!(out.contains("--token=***"));
    }

    #[test]
    fn masks_token_flag_with_space() {
        let out = sanitize_command_line("deploy --api-key supersecretvalue123");
        assert!(!out.contains("supersecretvalue123"));
        assert!(out.contains("--api-key ***"));
    }

    #[test]
    fn sanitize_command_joins_argv_and_masks() {
        let argv = vec![
            "git".to_string(),
            "clone".to_string(),
            "https://bob:hunter2@example.com/repo.git".to_string(),
        ];
        let out = sanitize_command(&argv);
        assert!(out.starts_with("git clone https://"));
        assert!(!out.contains("hunter2"));
    }

    #[test]
    fn sanitize_is_idempotent() {
        let once = sanitize_text("sk-abcdefghijklmnop12345678");
        let twice = sanitize_text(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn command_flag_masking_survives_audit() {
        // 保留 flag 与分隔符形态，审计仍能看懂命令结构
        let out = sanitize_command(&[
            "cargo".into(),
            "publish".into(),
            "--token".into(),
            "sk-abcdefghijklmnop12345678".into(),
        ]);
        assert!(out.contains("cargo publish --token ***"));
    }

    /// 环境变量值脱敏表（票 15 / §12.4.4）：按名判定，逐行锁定行为。
    #[test]
    fn masks_env_values_by_name_table() {
        let cases: &[(&str, &str)] = &[
            // 敏感名：赋值 / export / 引号 / 展开，一律打码
            ("API_TOKEN=abc123", "API_TOKEN=***"),
            ("export TOKEN=abc", "export TOKEN=***"),
            ("export API_KEY='sk-live-123'", "export API_KEY=***"),
            ("SECRET=\"a b c\"", "SECRET=***"),
            ("DB_PASSWORD=hunter2", "DB_PASSWORD=***"),
            ("MY_CREDENTIAL=x", "MY_CREDENTIAL=***"),
            // 变量展开：TOKEN 命中，HOME / FOO 不命中
            ("${TOKEN}", "${***}"),
            ("$TOKEN", "$***"),
            ("${HOME}/x", "${HOME}/x"),
            ("$FOO", "$FOO"),
            // 无害名（含 criterion 1 的 FOO=secret）：按名判定不误伤
            ("FOO=secret", "FOO=secret"),
            ("MODE=debug", "MODE=debug"),
            ("PATH=/usr/bin:/bin", "PATH=/usr/bin:/bin"),
            // 敏感名但无害值：纯数字保留（明确取舍，见 value_is_harmless）
            ("AUTH_TIMEOUT=30", "AUTH_TIMEOUT=30"),
            ("MAX_TOKENS=4096", "MAX_TOKENS=4096"),
            // 已 *** 幂等：再打一次结果不变
            ("TOKEN=***", "TOKEN=***"),
            ("export SECRET=***", "export SECRET=***"),
            // set -x 追踪：`+ NAME=value` 形态同样覆盖（前缀 `+ ` 原样保留）
            ("+ export API_TOKEN=abc123", "+ export API_TOKEN=***"),
            ("+ DB_PASSWORD=hunter2", "+ DB_PASSWORD=***"),
            // 已知误伤（契约已述）：含 AUTH / KEY 子串的无害名会被打码
            ("GIT_AUTHOR_NAME=Alice", "GIT_AUTHOR_NAME=***"),
        ];
        for (input, want) in cases {
            assert_eq!(&sanitize_text(input), want, "输入 {input:?}");
        }
    }

    /// 赋值模式的误伤边界：比较运算 / 无关文本不应被打码。
    #[test]
    fn env_assignment_does_not_mangle_comparisons() {
        // `token == y` 不是赋值（值首字符是 `=`）
        assert_eq!(
            sanitize_text("if token == y { ok }"),
            "if token == y { ok }"
        );
        // `--token=x` 由 flag 正则处理，不因赋值正则漏打
        let out = sanitize_command_line("run --token=abc123value");
        assert!(!out.contains("abc123value"));
        assert!(out.contains("--token=***"));
    }

    /// 敏感名 + 密钥形态组合：先按名打码，密钥形态正则不会残留片段。
    #[test]
    fn env_and_key_regexes_compose() {
        let out = sanitize_text("export GITHUB_TOKEN=sk-abcdefghijklmnop12345678");
        assert!(!out.contains("sk-abcdefghijklmnop12345678"));
        assert_eq!(out, "export GITHUB_TOKEN=***");
    }
}
