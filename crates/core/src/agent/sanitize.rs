//! 脱敏（决策 118，§12.4.4）。
//!
//! 时机是硬要求（决策 118）：输出脱敏在结果**回填 agent messages 之前**执行——
//! agent 看到的工具结果即脱敏后文本，落库与 context 同源。
//! 误伤面（合法长 base64 被打码）是已知代价。

use std::sync::OnceLock;

use regex::Regex;

const MASK: &str = "***";

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

/// 文本脱敏：密钥形态与长 base64 一律替换为 `***`。
pub fn sanitize_text(text: &str) -> String {
    let mut out = text.to_string();
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
}
