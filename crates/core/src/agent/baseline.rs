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
///
/// **只增不减**（§10.6.4）：节点级声明由调用方先并入 `declared`，本函数不削减任何一项
/// ——基线为空（决策 172①：内嵌技能退场），故实际等于 阶段级 ∪ 节点级。
///
/// **同名技能由更具体的一层决定**（`declared` 里阶段级在前、节点级在后）。这既贴合
/// `node_overrides_json` 的 override 语义，也与同表的 `idle_timeout_sec` 一致
/// （节点级 > 阶段级 > 全局，决策 66）——节点级声明 `mode: "name"` 却仍被阶段级的
/// `full` 压住，等于让「节点专属技能」这个功能对同名技能失效。保留**首次出现的位置**，
/// 使渲染顺序与声明顺序一致。这不违反「只增不减」：技能仍在集合里，变的只是注入形态。
///
/// 注意覆盖方向**不能**绕过信任门：未信任 + `full` 的声明在 [`crate::config::parse_skill_decls`]
/// 就被拒绝，走不到这里。
///
/// 基线 [`BASELINE_MANDATORY_SKILLS`] 目前为空（决策 172①：内嵌技能退场，推荐默认改由
/// 配置界面承载），但仍按并集语义参与——与 [`effective_tools`] 对
/// [`BASELINE_MANDATORY_TOOLS`] 的处理对称，保住「mandatory 不可移除」这条机制
/// （§10.6.2 的约束语义同样覆盖技能）。
pub fn effective_skills(
    declared: &[crate::agent::skills::SkillDecl],
) -> Vec<crate::agent::skills::SkillDecl> {
    use crate::agent::skills::SkillDecl;
    let mut out: Vec<SkillDecl> = BASELINE_MANDATORY_SKILLS
        .iter()
        .map(|n| SkillDecl::from_bare(*n))
        .collect();
    for d in declared {
        match out.iter_mut().find(|s| s.name == d.name) {
            // 后声明的（更具体的一层）改写形态，位置不变
            Some(existing) => {
                existing.mode = d.mode;
                existing.trusted = d.trusted;
            }
            None => out.push(d.clone()),
        }
    }
    out
}

/// 有效技能名（启动校验与 `PUT /stage-configs` 的存在性检查用）。
pub fn effective_skill_names(declared: &[crate::agent::skills::SkillDecl]) -> Vec<String> {
    effective_skills(declared)
        .into_iter()
        .map(|d| d.name)
        .collect()
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
        use crate::agent::skills::SkillDecl;
        assert!(effective_skills(&[]).is_empty());
        assert_eq!(
            effective_skills(&[SkillDecl::from_bare("rtk")]),
            vec![SkillDecl::from_bare("rtk")]
        );
        assert_eq!(
            effective_skill_names(&[SkillDecl::from_bare("rtk")]),
            vec!["rtk".to_string()]
        );
    }

    /// 决策 172④：节点级声明**只增不减**，同名技能保留首次出现的形态
    /// （阶段级在前 → 节点级无法改写其 mode/trusted，也不能削减它）。
    #[test]
    fn effective_skills_is_union_and_more_specific_wins() {
        use crate::agent::skills::{SkillDecl, SkillMode};
        let node_level = SkillDecl {
            name: "grilling".into(),
            mode: SkillMode::Name,
            trusted: true,
        };
        let got = effective_skills(&[
            SkillDecl::from_bare("grilling"), // 阶段级：full
            node_level.clone(),               // 节点级同名：改写形态
            SkillDecl::from_bare("to-spec"),
        ]);
        assert_eq!(got.len(), 2, "同名去重且不削减：{got:?}");
        assert_eq!(got[0].name, "grilling");
        // 位置保持首次出现处，形态取更具体的一层
        assert_eq!(got[0].mode, SkillMode::Name, "节点级形态应生效");
        assert!(got[0].trusted);
        assert_eq!(got[1].name, "to-spec", "其余技能保持声明顺序");
    }

    /// 反向顺序（节点级在前）不应「反被阶段级压回」——覆盖只由**更具体的一层**决定，
    /// 而调用方保证了阶段级在前。这里钉住「后出现的赢」这条机械规则，避免实现漂移。
    #[test]
    fn effective_skills_last_declaration_wins() {
        use crate::agent::skills::{SkillDecl, SkillMode};
        let stage_level = SkillDecl {
            name: "x".into(),
            mode: SkillMode::Full,
            trusted: true,
        };
        let got = effective_skills(&[SkillDecl::from_bare("x"), stage_level]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].mode, SkillMode::Full, "后声明者改写形态");
        assert!(got[0].trusted);
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
