//! 管线内置工具的**规格表**（决策 353）：8 个内置工具的名字、广告语与参数 schema 一张表。
//!
//! 「一个工具是什么」此前散在五处（client.rs 名单、tools.rs 层名单 + dispatch、
//! model_request 的 tool_defs、prompts 散文），而 tool_defs 广告出去的是空壳
//! （`description: String::new()` + `{"type":"object"}`）——`edit_file` 的
//! old_text/new_text 契约在 [`ToolExecutor::execute`] 有硬校验与报错信息，却不在
//! 广告出的接口里，模型只能从散文猜参数形状。本表收口之后，加一个工具的触碰面 =
//! 这里一行（再按冻结断言的指引进对名单与 dispatch）。
//!
//! 吃这张表的：
//! - [`crate::pipeline::model_request`] 的 `tool_defs`（广告集）与子代理的固定只读定义；
//! - [`super::tools::ToolExecutor::execute`] 的 dispatch（名字引用本表常量）。
//!
//! **没有「层」字段**（决策 247 删掉手标枚举的理由同样适用于这里）：「会改动东西」
//! 由档位谓词（`is_env_write_tool` ∨ `is_service_write_tool`）判，分层对应由 tests 里的
//! 冻结断言钉住——断言是纯校验，不是第二份知识。
//!
//! **`submit_metadata` 的形状是占位**：它的参数 schema 随节点种类由对应 Rust 结构体
//! 派生（决策 38，与校验同源），本表不复制第二份——[`def_for`] 对这个名字返回
//! `None`（用例钉住），`tool_defs` 对它走 [`super::client::submit_metadata_tool`]。

use super::client::ToolDef;

// ── 名字常量：dispatch 与表共用一份字面量（决策 353：dispatch 名字引用目录表）──────

pub const WRITE_FILE: &str = "write_file";
pub const EDIT_FILE: &str = "edit_file";
pub const READ_FILE: &str = "read_file";
pub const DELETE_FILE: &str = "delete_file";
pub const LIST_DIR: &str = "list_dir";
pub const RUN_COMMAND: &str = "run_command";
pub const SUBMIT_METADATA: &str = "submit_metadata";
/// `Skill` 与上游同名是**功能性决定**（决策 172③），字面量的主人仍是 [`super::client::SKILL_TOOL`]。
pub const SKILL: &str = super::client::SKILL_TOOL;
/// 重活外发（票 runner-offload/06）：agent 把一条白名单 cargo 命令交给 GitHub Actions。
pub const OFFLOAD_RUN: &str = "offload_run";

/// 一个内置工具的规格：名字 + 广告语 + 参数 JSON-Schema。
pub struct ToolSpec {
    pub name: &'static str,
    /// 广告给模型的话。**必填非空**（用例钉住）：空壳广告是决策 353 要退场的东西。
    pub description: &'static str,
    /// 参数 JSON-Schema 的**文本**：常量表里放不了 `serde_json::Value`，用文本 + 一处
    /// 解析（[`def_for`]），单测钉住它是合法 JSON（与 `ForemanToolSpec` 同一姿态）。
    pub parameters: &'static str,
}

/// 9 个内置工具（决策 353；9 = 8 + `offload_run`，票 runner-offload/06）。
/// 顺序与 [`super::client::BUILTIN_TOOLS`] 一致（冻结断言逐位钉住）。
pub const TOOL_SPECS: [ToolSpec; 9] = [
    ToolSpec {
        name: WRITE_FILE,
        description: "把整份内容写入任务工作区里的一个文件（整份覆盖，路径相对工作区根）。\
                      改之前先 read_file 看一眼——覆盖不可逆，没读过的内容会被抹掉。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对工作区根的路径"},"content":{"type":"string","description":"文件的全部内容（整份覆盖）"}},"required":["path","content"]}"#,
    },
    ToolSpec {
        name: EDIT_FILE,
        description: "把文件里的一处原文换成新文本：old_text 必须与文件中的原文逐字一致\
                      （只替换第一处），且不能是空串。只想动一小段时用它，不要整份重写。\
                      改之前先 read_file。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对工作区根的路径"},"old_text":{"type":"string","description":"要被替换的原文（须与文件中的原文逐字一致；只替换第一处；不能是空串）"},"new_text":{"type":"string","description":"替换成什么（空串即删除该段）"}},"required":["path","old_text","new_text"]}"#,
    },
    ToolSpec {
        name: READ_FILE,
        description: "读任务工作区里的一个文件（路径相对工作区根）。默认读头部并带结构大纲；\
                      offset / limit 控制行窗，tail=true 读尾部——日志与运行记录这类\
                      追加写的文件用它。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对工作区根的路径"},"offset":{"type":"integer","description":"起始行（从 0 数）；tail 为真时忽略"},"limit":{"type":"integer","description":"最多读几行"},"tail":{"type":"boolean","description":"读尾部而不是头部（日志用它）"}},"required":["path"]}"#,
    },
    ToolSpec {
        name: DELETE_FILE,
        description: "删除任务工作区里的一个文件。不存在视为已删除（幂等）。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对工作区根的路径"}},"required":["path"]}"#,
    },
    ToolSpec {
        name: LIST_DIR,
        description: "列一个目录的内容（路径相对工作区根，省略取根）。recursive=true 递归列出。\
                      不知道文件在哪儿时先列一层。",
        parameters: r#"{"type":"object","properties":{"path":{"type":"string","description":"相对工作区根的路径（省略取根）"},"recursive":{"type":"boolean","description":"是否递归"}}}"#,
    },
    ToolSpec {
        name: RUN_COMMAND,
        description: "在任务工作区里跑一条 shell 命令：cwd 指定工作目录（缺省工作区根），\
                      timeout_sec 覆盖超时。输出作为工具回执返回，并落进任务的命令台账；\
                      会不会立即执行由环境权限档位决定（ask 档转提议等人确认）。",
        parameters: r#"{"type":"object","properties":{"command":{"type":"string","description":"要执行的命令原文"},"cwd":{"type":"string","description":"工作目录（缺省工作区根）"},"timeout_sec":{"type":"integer","description":"超时秒数"}},"required":["command"]}"#,
    },
    ToolSpec {
        name: SUBMIT_METADATA,
        description: "提交本节点的结构化元数据——结构化流转的唯一出口，不要在正文里夹带 JSON。\
                      参数 schema 随节点种类由对应 Rust 结构体派生（决策 38，与校验同源），\
                      以本次工具定义给出的 schema 为准；本表不复制它。",
        parameters: r#"{"type":"object"}"#,
    },
    ToolSpec {
        name: SKILL,
        description: "按名字加载一个技能的正文（技能目录里列出的名字）。\
                      上游技能正文里的 `Call the Skill tool` 说的就是这个工具。",
        parameters: r#"{"type":"object","properties":{"name":{"type":"string","description":"技能名（见 system prompt 的技能目录）"}},"required":["name"]}"#,
    },
    ToolSpec {
        name: OFFLOAD_RUN,
        description: "把一条重活命令外发给 GitHub Actions 跑（设置里「重活外发」开着才可用）。\
                      只收 cargo test / cargo clippy / cargo build 前缀的命令，且只外发\
                      **已提交**状态：当前分支会推到远端，工作区必须干净。适合等得起几分钟的\
                      全量测试 / lint；快命令用 run_command。外发链路本身出问题会自动回退\
                      本机执行并说明；远端命令失败会带回退出码与日志尾部。",
        parameters: r#"{"type":"object","properties":{"command":{"type":"string","description":"要外发的命令（仅 cargo test / cargo clippy / cargo build 前缀；不含 ; & | ` 换行 重定向 等组合符）"}},"required":["command"]}"#,
    },
];

/// 按名字查规格（广告集与 dispatch 共用的判据入口）。
pub fn spec_for(name: &str) -> Option<&'static ToolSpec> {
    TOOL_SPECS.iter().find(|s| s.name == name)
}

/// 规格 → 广告给模型的 [`ToolDef`]（schema 文本在此解析，一次一处）。
///
/// [`SUBMIT_METADATA`] **不在这条出口上**：它的参数 schema 随节点种类派生
/// （决策 38 与校验同源），喂出本表的占位就是假 schema——`tool_defs` 对它走
/// [`super::client::submit_metadata_tool`]（用例钉住本返回 `None`）。
pub fn def_for(name: &str) -> Option<ToolDef> {
    if name == SUBMIT_METADATA {
        return None;
    }
    let spec = spec_for(name)?;
    Some(ToolDef {
        name: spec.name.to_string(),
        description: spec.description.to_string(),
        parameters: serde_json::from_str(spec.parameters)
            .expect("目录表里的参数 schema 必须是合法 JSON（单测钉住）"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::client::{BUILTIN_TOOLS, SKILL_TOOL};
    use crate::agent::tools::{is_env_tool, is_env_write_tool, is_service_write_tool};

    /// 冻结断言（决策 353）：目录名字集 = `BUILTIN_TOOLS`，逐位一致——两份名单谁漂了
    /// 这条都红。纯校验，不是第二份知识。
    #[test]
    fn catalog_names_equal_builtin_tools_position_for_position() {
        let names: Vec<&str> = TOOL_SPECS.iter().map(|s| s.name).collect();
        assert_eq!(names, BUILTIN_TOOLS.to_vec());
        assert_eq!(spec_for(SKILL_TOOL).unwrap().name, SKILL_TOOL);
    }

    /// 冻结断言（决策 353）：目录名字集与三份层名单的**分层对应**——
    /// 内置 9 个里除 `submit_metadata`（本服务读结构化结论，不碰档位）与 `offload_run`
    /// （决策 381：它推的是远端白名单命令，不走环境写层；可用性由设置里的
    /// 外发开关管，广告侧跟着开关走——见 `model_request` 的广告点；deny 档在
    /// 执行点另拦）外全在 [`is_env_tool`] 层；动手的那四个恰是 `ENV_WRITE_TOOLS`
    /// 与内置集的交；没有任何一个内置工具是本服务写接口。层字段不进目录表
    /// （决策 247），这份对应就是「表 × 名单」之间唯一的对账单。
    #[test]
    fn frozen_layering_between_catalog_and_tier_lists() {
        let names: Vec<&str> = TOOL_SPECS.iter().map(|s| s.name).collect();
        for name in &names {
            if *name == SUBMIT_METADATA || *name == OFFLOAD_RUN {
                assert!(!is_env_tool(name), "{name} 不该在环境层");
                continue;
            }
            assert!(is_env_tool(name), "{name} 应在 ENV_TOOLS 层");
            assert!(!is_service_write_tool(name), "{name} 不该是本服务写接口");
        }
        let env_writes: Vec<&str> = names
            .iter()
            .copied()
            .filter(|n| is_env_write_tool(n))
            .collect();
        assert_eq!(
            env_writes,
            vec![WRITE_FILE, EDIT_FILE, DELETE_FILE, RUN_COMMAND],
            "内置集里「动手」的恰是这四个（与 ENV_WRITE_TOOLS 的交）"
        );
    }

    /// 目录表里的 schema 文本全是合法 JSON object（[`def_for`] 的 expect 靠它兜底）。
    #[test]
    fn all_parameter_texts_are_valid_json_objects() {
        for spec in &TOOL_SPECS {
            let v: serde_json::Value = serde_json::from_str(spec.parameters)
                .unwrap_or_else(|e| panic!("{} 的 parameters 不是合法 JSON：{e}", spec.name));
            assert!(v.is_object(), "{} 的 parameters 须是 object", spec.name);
        }
    }

    /// 抽查断言（决策 353 验收线）：每个工具**广告出的**参数 schema 与
    /// `execute()` 实际解析的字段逐字段一致——properties 的键集与 required 都是手写的
    /// 期望值，对着 [`ToolExecutor::execute`] 各分支的 `args.get(...)` 读数。
    #[test]
    fn advertised_schemas_match_execute_parsing_field_by_field() {
        let expected: [(&str, &[&str], &[&str]); 8] = [
            (WRITE_FILE, &["path", "content"], &["path", "content"]),
            (
                EDIT_FILE,
                &["path", "old_text", "new_text"],
                &["path", "old_text", "new_text"],
            ),
            (READ_FILE, &["path", "offset", "limit", "tail"], &["path"]),
            (DELETE_FILE, &["path"], &["path"]),
            (LIST_DIR, &["path", "recursive"], &[]),
            (
                RUN_COMMAND,
                &["command", "cwd", "timeout_sec"],
                &["command"],
            ),
            (SKILL, &["name"], &["name"]),
            (OFFLOAD_RUN, &["command"], &["command"]),
        ];
        for (name, props, required) in &expected {
            let def = def_for(name).unwrap_or_else(|| panic!("{name} 应有目录行"));
            let schema = &def.parameters;
            let mut got_props: Vec<&str> = schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("{name} 须有 properties"))
                .keys()
                .map(|k| k.as_str())
                .collect();
            got_props.sort_unstable();
            let mut want_props = props.to_vec();
            want_props.sort_unstable();
            assert_eq!(got_props, want_props, "{name} 的 properties 漂了");

            let mut got_required: Vec<&str> = schema["required"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            got_required.sort_unstable();
            let mut want_required = required.to_vec();
            want_required.sort_unstable();
            assert_eq!(got_required, want_required, "{name} 的 required 漂了");
        }
    }

    /// 广告非空（空壳 `description: String::new()` 是决策 353 退场的东西）。
    #[test]
    fn every_description_is_non_empty() {
        for spec in &TOOL_SPECS {
            assert!(
                !spec.description.trim().is_empty(),
                "{} 的广告语为空",
                spec.name
            );
        }
    }

    /// `submit_metadata` 的形状出口只有 [`super::client::submit_metadata_tool`] 一处
    /// （决策 38 同源）：目录行是登记 + 广告语，[`def_for`] 对它返回 `None`，
    /// 占位 schema 永远不该被广告出去。
    #[test]
    fn def_for_never_serves_the_submit_metadata_placeholder() {
        assert!(def_for(SUBMIT_METADATA).is_none());
        // 其余 7 个照常出表。
        for spec in &TOOL_SPECS {
            if spec.name == SUBMIT_METADATA {
                continue;
            }
            assert!(def_for(spec.name).is_some(), "{} 应能出表", spec.name);
        }
    }
}
