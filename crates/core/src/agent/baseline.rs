//! 系统最小基线（§10.6.2，G6 / 决策 104）。
//!
//! 约束语义：阶段配置只能让 agent **能力更强或更聚焦**，不能绕过系统保障——
//! 工具集 = `(mandatory ∪ 阶段声明) − forbidden`，mandatory 不可移除。

/// 不可移除的基线工具（决策 45）。与 [`crate::agent::client::MANDATORY_TOOLS`] 同源。
pub use crate::agent::client::MANDATORY_TOOLS as BASELINE_MANDATORY_TOOLS;

/// 禁止工具（§10.6.2）。v1 为空：决策 19（修订）明确 run_command 的 shell 不受限，
/// 文件工具的路径约束由 [`crate::agent::file_policy::FileToolPolicy`] 承担。
pub const BASELINE_FORBIDDEN_TOOLS: [&str; 0] = [];

/// 强制注入的 skill（§10.6.2）。默认空，全部走用户配置。
pub const BASELINE_MANDATORY_SKILLS: [&str; 0] = [];

/// 有效工具集：`(基线 mandatory ∪ 阶段声明) − forbidden`，去重且保持
/// mandatory 顺序在前（§10.6.4 合并规则）。
pub fn effective_tools(declared: &[String]) -> Vec<String> {
    effective_set(
        &BASELINE_MANDATORY_TOOLS,
        &BASELINE_FORBIDDEN_TOOLS,
        declared,
    )
}

/// 有效 skill 集：`(基线 mandatory ∪ 阶段声明)`（引用不存在 skill 的 fail fast
/// 在启动校验 [`crate::config::validate_startup`]，这里只做并集）。
pub fn effective_skills(declared: &[String]) -> Vec<String> {
    effective_set(&BASELINE_MANDATORY_SKILLS, &[], declared)
}

fn effective_set(mandatory: &[&str], forbidden: &[&str], declared: &[String]) -> Vec<String> {
    let mut out: Vec<String> = mandatory.iter().map(|s| s.to_string()).collect();
    for d in declared {
        if !out.contains(d) {
            out.push(d.clone());
        }
    }
    out.retain(|t| !forbidden.contains(&t.as_str()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn baseline_alone_yields_all_mandatory_tools() {
        assert_eq!(effective_tools(&[]), declared(&BASELINE_MANDATORY_TOOLS));
        assert!(BASELINE_MANDATORY_TOOLS.contains(&"submit_metadata"));
    }

    #[test]
    fn declared_tools_extend_mandatory_set() {
        let got = effective_tools(&declared(&["web_search", "read_file"]));
        assert!(got.contains(&"web_search".to_string()));
        // read_file 已在 mandatory 里，不重复
        assert_eq!(got.iter().filter(|t| *t == "read_file").count(), 1);
        // mandatory 一个不少
        for m in BASELINE_MANDATORY_TOOLS {
            assert!(got.contains(&m.to_string()));
        }
    }

    #[test]
    fn declared_cannot_remove_mandatory() {
        // 阶段声明里"去掉"submit_metadata 是不可能表达的——并集语义保证它仍在
        let got = effective_tools(&declared(&["read_file", "run_command"]));
        assert!(got.contains(&"submit_metadata".to_string()));
        assert!(got.contains(&"write_file".to_string()));
    }

    #[test]
    fn forbidden_tools_are_filtered_after_union() {
        // 机制测试：即使未来 forbidden 收紧，并集结果也必须先剔除
        let got = effective_set(
            &["submit_metadata", "read_file", "run_command"],
            &["run_command"],
            &declared(&["run_command", "web_search"]),
        );
        assert!(!got.contains(&"run_command".to_string()));
        assert!(got.contains(&"submit_metadata".to_string()));
        assert!(got.contains(&"web_search".to_string()));
    }

    #[test]
    fn skills_union_with_empty_baseline() {
        assert!(effective_skills(&[]).is_empty());
        assert_eq!(effective_skills(&declared(&["rtk"])), declared(&["rtk"]));
    }

    #[test]
    fn forbidden_never_overlaps_mandatory() {
        // §10.6.4：forbidden 剔除发生在并集之后，若与 mandatory 相交会静默移除
        // 强制工具——基线常量必须保证二者不相交（否则 effective_tools 无错误通道）。
        for f in BASELINE_FORBIDDEN_TOOLS {
            assert!(!BASELINE_MANDATORY_TOOLS.contains(&f));
        }
    }
}
