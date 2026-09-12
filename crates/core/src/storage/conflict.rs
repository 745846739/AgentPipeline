//! 第一层冲突比对（决策 53 / 60 / 71 / 102 / 120）。
//!
//! 活跃任务 = `status ∈ {running, pending, waiting, queued}`；符号判重用
//! `(module_path, name)`；纯 `name` 重合只产生 warning；**环消除**：只有 `created_at`
//! 较晚者让步（同秒以 id 字典序大者让步）。

use std::collections::HashSet;

use super::tasks::TaskFilter;
use super::Store;
use crate::types::{ConflictWarning, DuplicateRisk, NewSymbol, NodeCursor, Task};
use crate::Result;

impl Store {
    /// 读任务的 architect 产出元数据，归一化成比对键。
    pub async fn overlap_keys(&self, task_id: &str) -> Result<OverlapKeys> {
        let meta = self
            .stage_output_metadata(task_id, crate::types::Stage::ArchitectDesign, "design_doc")
            .await?;
        let Some(meta) = meta else {
            return Ok(OverlapKeys::default());
        };
        let files: Vec<String> = meta
            .get("affected_files")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(normalize_path)
                    .collect()
            })
            .unwrap_or_default();
        let symbols: Vec<(String, String)> = meta
            .get("new_symbols")
            .and_then(|v| serde_json::from_value::<Vec<NewSymbol>>(v.clone()).ok())
            .map(|syms| syms.into_iter().map(|s| (s.module_path, s.name)).collect())
            .unwrap_or_default();
        Ok(OverlapKeys { files, symbols })
    }

    /// 第一层冲突比对：返回**本任务需要让步**的冲突警告（决策 71 ③ 的环消除已应用）。
    ///
    /// `Ok(空)` 表示无冲突或本任务先到（不因对方回退）。
    pub async fn first_layer_conflicts(&self, task_id: &str) -> Result<Vec<ConflictWarning>> {
        let me = self.get_task(task_id).await?;
        let mine = self.overlap_keys(task_id).await?;
        let mut warnings = Vec::new();

        for other in self
            .list_tasks(&TaskFilter {
                include_archived: false,
                ..Default::default()
            })
            .await?
        {
            if other.id == me.id || !other.status.is_active() {
                continue;
            }
            let theirs = self.overlap_keys(&other.id).await?;
            let overlapping_files: Vec<String> = intersect(&mine.files, &theirs.files);
            let overlapping_symbols: Vec<String> = mine
                .symbols
                .iter()
                .filter(|s| theirs.symbols.contains(s))
                .map(|(module, name)| format!("{module}::{name}"))
                .collect();
            // 纯 name 重合不算冲突，只进 warning（决策 71 ②）
            let pure_name_overlap: Vec<String> = mine
                .symbols
                .iter()
                .filter(|(_, name)| theirs.symbols.iter().any(|(_, n)| n == name))
                .map(|(module, name)| format!("{module}::{name}"))
                .collect();

            if overlapping_files.is_empty() && overlapping_symbols.is_empty() {
                // 决策 71② / 120：纯 name 重合（不同 module_path）不触发 conflict_wait，
                // 只降级为 warning（Low）。warning 不引发等待，不做环消除、双向都可见。
                if !pure_name_overlap.is_empty() {
                    warnings.push(ConflictWarning {
                        task_id: other.id.clone(),
                        task_title: other.title.clone(),
                        overlapping_files: Vec::new(),
                        overlapping_symbols: pure_name_overlap,
                        duplicate_risk: Some(DuplicateRisk::Low),
                    });
                }
                continue;
            }
            // 环消除：只有较晚创建者让步（同秒 id 字典序大者让步）
            if !self.yields_to(&me, &other) {
                continue;
            }
            warnings.push(ConflictWarning {
                task_id: other.id.clone(),
                task_title: other.title.clone(),
                overlapping_files,
                overlapping_symbols,
                duplicate_risk: Some(DuplicateRisk::High),
            });
        }
        Ok(warnings)
    }

    /// 复检（决策 102）：恢复前用已有产出重跑第一层比对，仍冲突的 id 列表。
    pub async fn recheck_first_layer_overlap(&self, cursor: &NodeCursor) -> Result<Vec<String>> {
        // 复检不再应用环消除——此时只需要"是否还有交集"
        let mine = self.overlap_keys(&cursor.task_id).await?;
        let mut conflicts = Vec::new();
        for other in self
            .list_tasks(&TaskFilter {
                include_archived: false,
                ..Default::default()
            })
            .await?
        {
            if other.id == cursor.task_id || !other.status.is_active() {
                continue;
            }
            let theirs = self.overlap_keys(&other.id).await?;
            if !intersect(&mine.files, &theirs.files).is_empty()
                || mine.symbols.iter().any(|s| theirs.symbols.contains(s))
            {
                conflicts.push(other.id);
            }
        }
        conflicts.sort();
        Ok(conflicts)
    }

    /// `me` 是否为较晚者（较晚者让步）。
    pub fn yields_to(&self, me: &Task, other: &Task) -> bool {
        (me.created_at, me.id.as_str()) > (other.created_at, other.id.as_str())
    }
}

/// 归一化后的比对键。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OverlapKeys {
    pub files: Vec<String>,
    pub symbols: Vec<(String, String)>,
}

fn intersect(a: &[String], b: &[String]) -> Vec<String> {
    let set: HashSet<&String> = b.iter().collect();
    let mut out: Vec<String> = a
        .iter()
        .filter(|item| set.contains(item))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

/// 路径归一化（决策 53：比较规范化后的集合）。
pub fn normalize_path(path: &str) -> String {
    path.trim_start_matches("./")
        .trim_end_matches('/')
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_normalization() {
        assert_eq!(normalize_path("./src/a.rs"), "src/a.rs");
        assert_eq!(normalize_path("src/b/"), "src/b");
        assert_eq!(normalize_path("src\\c.rs"), "src/c.rs");
    }

    #[test]
    fn intersect_dedups_and_sorts() {
        let a = vec!["b".to_string(), "a".to_string(), "a".to_string()];
        let b = vec!["a".to_string(), "c".to_string()];
        assert_eq!(intersect(&a, &b), vec!["a".to_string()]);
    }
}
