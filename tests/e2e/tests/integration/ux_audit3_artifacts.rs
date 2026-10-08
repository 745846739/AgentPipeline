//! L4 产出物验收：第三轮 UX 审计（`test-scenarios.md` 场景 1–15）的机器可检断言。
//!
//! **被测对象**是本任务的产出物（`.scratch/ux-audit-3/{README.md, issues/**}`、
//! `frontend/e2e/ux-audit-3.spec.ts`、`.gitignore` 一行），不是产品运行时行为——
//! design.md §2 边界与 AC-7 明令 `frontend/src/**`、`crates/**` 零 diff，
//! 所以测试落点必须在这两者之外；`tests/e2e` 是工作区里唯一满足该边界的 cargo
//! 测试目标，且它自己的 `main.rs` 头就写着「加新场景：放文件进本目录 + 加一行 mod」。
//!
//! 场景 → 用例对照（每个场景至少一条）：
//!
//! | 场景 | 用例 |
//! |---|---|
//! | 1 骨架先落盘（AC-1） | `scene_01_index_skeleton_lands_with_placeholders_before_backfill` |
//! | 2 四栏齐备、集合相等（AC-2） | `scene_02_every_candidate_has_a_body_with_four_fields` |
//! | 3 点名三条有结论有判据（AC-3） | `scene_03_three_named_rechecks_have_conclusions_and_criteria` |
//! | 4 证据二选一可追（AC-5） | `scene_04_evidence_levels_are_traceable_with_source_spot_checks` |
//! | 5 与前轮关联可解析、wontfix 不复活（AC-6） | `scene_05_prior_round_links_resolve_and_wontfix_stays_closed` |
//! | 6 产品代码零改动（AC-7） | `scene_06_product_code_zero_diff_and_sole_allowed_changes` |
//! | 7 spec 存在且默认 skip（AC-4） | `scene_07_new_spec_exists_and_skips_by_default` |
//! | 8 设 env 真跑打数字（AC-4，静态前半） | `scene_08_readings_and_screenshots_are_citable` |
//! | 9 既有 spec 零 diff（AC-3/7） | `scene_09_prior_audit_specs_untouched` |
//! | 10 README 可复跑（AC-8） | `scene_10_readme_lists_reproducible_commands_and_unverified_items` |
//! | 11 候选池冻结三源（AC-1/2） | `scene_11_candidate_pool_frozen_to_three_sources` |
//! | 12 正文只到建议层（AC-2） | `scene_12_body_shape_stops_at_suggestion_layer` |
//! | 13 截图不入库（AC-7） | `scene_13_png_ignored_markdown_tracked` |
//! | 14 造不出的档不冒充实测（AC-5/8） | `scene_14_unfalsifiable_states_never_claimed_as_measured` |
//! | 15 中断点上都有自足清单（AC-1/2） | `scene_15_interruptible_selfcontained_checklist` |
//!
//! **三处口径说明（详见 test-report.md 的命令台账）**：
//!
//! 1. **git 历史断言带可用性守卫**：`rev-list --count HEAD < 5`、
//!    `merge-base HEAD origin/main` 失败（CI 的 depth=1 checkout，见 `docs/testing.md` §6
//!    与 `runner_for_doc.rs` 文件头的同源口径），merge-base 与 HEAD 同点
//!    （审计已合入 main，`mb...HEAD` 退化为空集），或**审计产物已在 `origin/main`**
//!    （审计轮已收口，当前分支是后续按设计要改产品的任务分支——把审计当轮的
//!    「产品零 diff + 白名单 + .gitignore +2 行」点断言套上去必然假红）时，
//!    相应子断言跳过并打 stdout 说明——完整历史 + 审计轮当轮才跑实断言。
//! 2. **场景 1 的「严格早于」是更紧读法**：骨架提交 `53a9eb9` 一次落
//!    `00-INDEX.md` + 13 个 `待填` 空壳（同秒入库），`d6b9311` 起逐条回填——
//!    design AC-1 的原话是「在某条正文**补齐**之前已 commit/可见」，占位→回填链可证；
//!    评审报告已裁定「场景 1 严格读法属测试文档口径比设计更严，非产出物缺陷」。
//!    本用例按设计口径断言（骨架含占位、正文不早于骨架、回填在更晚的提交、现版无占位）。
//! 3. **场景 6 白名单扩一行给本阶段自身**：`tests/e2e/tests/integration/**`
//!    （本测试文件与 `main.rs` 的 mod 行）是 test 阶段的硬产出，不是审计产出；
//!    除此前缀外，分支 diff 必须恰落场景 6 的三处白名单，`frontend/src` + `crates`
//!    字面零 diff（AC-7 点名口径原样断言）。工作区清洁（`git status --porcelain`）
//!    与场景 7/8/9 的 Playwright 实跑属流程/运行证据，记 test-report.md 台账。
//! 4. **跨轮链接按双锚判定可达**（场景 5 第 2 步「点名到的前轮票号全部可达」）：
//!    语料里 `../ux-audit-2/…` 沿用 README（位于轮次根）的锚点，`../README.md`
//!    沿用票文件自身目录的锚点——两种写法在语料里都真实可达，故链接目标只要在
//!    「票文件目录」或「轮次根」任一锚下解析到真实文件即算可达（被点名的票文件
//!    本身必须存在，这由 `=前轮 NN` 的票号存在性断言单独硬证）。观察项见报告。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

const ISSUES_DIR: &str = ".scratch/ux-audit-3/issues";
const INDEX_REL: &str = ".scratch/ux-audit-3/issues/00-INDEX.md";
const AUDIT_README_REL: &str = ".scratch/ux-audit-3/README.md";
const SPEC_REL: &str = "frontend/e2e/ux-audit-3.spec.ts";
/// 历史断言的可用性下限：骨架提交 `53a9eb9` 起需要 5 个提交可见。
const MIN_HISTORY: usize = 5;

// ─────────────────────────── 基础设施 ───────────────────────────

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("CARGO_MANIFEST_DIR/../.. 应可规范化为仓库根")
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("读取 {rel} 失败：{e}"))
}

fn git_raw(root: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?} 无法启动：{e}"));
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let (ok, stdout) = git_raw(root, args);
    ok.then_some(stdout)
}

/// 全历史可用（CI 的 depth=1 checkout 下为 false，守卫见文件头）。
fn history_available(root: &Path) -> bool {
    git(root, &["rev-list", "--count", "HEAD"])
        .and_then(|s| s.trim().parse::<usize>().ok())
        .is_some_and(|n| n >= MIN_HISTORY)
}

fn merge_base(root: &Path) -> Option<String> {
    git(root, &["merge-base", "HEAD", "origin/main"]).map(|s| s.trim().to_string())
}

/// issues/ 下的一票一文件（NN-slug.md，排除 00-INDEX.md），按编号排序。
fn body_rels(root: &Path) -> Vec<String> {
    let mut rels: Vec<String> = std::fs::read_dir(root.join(ISSUES_DIR))
        .unwrap_or_else(|e| panic!("读 {ISSUES_DIR} 目录失败：{e}"))
        .map(|e| {
            e.expect("目录项")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| {
            name.ends_with(".md") && name != "00-INDEX.md" && name.as_bytes()[0].is_ascii_digit()
        })
        .map(|name| format!("{ISSUES_DIR}/{name}"))
        .collect();
    rels.sort();
    rels
}

/// 取 `**标签:**` 同一行的值（trim 后），没有该标签则 None。
fn field(body: &str, label: &str) -> Option<String> {
    let needle = format!("**{label}:**");
    let start = body.find(&needle)? + needle.len();
    let line = body[start..].split('\n').next().unwrap_or("");
    Some(line.trim().to_string())
}

fn rel_field(body: &str) -> Option<String> {
    field(body, "与前轮关联").or_else(|| field(body, "与前轮的关系"))
}

/// 「结论：」行里四档（已修/未修/回归/有意不做）最先出现的那一档。
fn conclusion_bucket(body: &str) -> Option<&str> {
    let line = body
        .lines()
        .find(|l| l.trim_start().starts_with("结论："))?;
    ["已修", "未修", "回归", "有意不做"]
        .into_iter()
        .filter_map(|tok| line.find(tok).map(|idx| (idx, tok)))
        .min_by_key(|(idx, _)| *idx)
        .map(|(_, tok)| tok)
}

/// 正文里有没有 `路径:行号` 形状的源码判据（AC-5 的「代码」级形态）。
fn has_source_ref(text: &str) -> bool {
    [".svelte:", ".ts:", ".rs:", ".css:"]
        .into_iter()
        .any(|ext| {
            text.match_indices(ext).any(|(i, _)| {
                text[i + ext.len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
            })
        })
}

/// 正文里点名前两轮的相对链接（`]((../ux-audit…`），供可达性断言。
fn prior_links(body: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut from = 0;
    while let Some(i) = body[from..].find("](../ux-audit") {
        let open = from + i + 1;
        let Some(close_rel) = body[open..].find(')') else {
            break;
        };
        links.push(body[open + 1..open + close_rel].to_string());
        from = open + close_rel + 1;
    }
    links
}

/// 证据等级行里出现的 `r3-*.png` 截图名（token 限定 ASCII，避免把
/// 「`r3-半句草稿`」这类非截图串误吞）。
fn cited_pngs(text: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("r3-") {
        let i = from + i;
        let mut n = 0;
        for ch in text[i..].chars() {
            if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' {
                n += ch.len_utf8();
            } else {
                break;
            }
        }
        let end = i + n;
        if text[end..].starts_with(".png") {
            set.insert(format!("{}.png", &text[i..end]));
            from = end + 4;
        } else {
            from = i + 3;
        }
    }
    set
}

/// spec 里 `import … from './<module>'` 的名字列表（`type X` 剥前缀，花括号剥外壳）。
fn imported_from(spec: &str, module: &str) -> Vec<String> {
    let pat = format!("from './{module}'");
    let at = spec
        .find(&pat)
        .unwrap_or_else(|| panic!("spec 应从 './{module}' 导入"));
    let before = &spec[..at];
    let imp = before.rfind("import").expect("import 关键字应在 from 之前");
    let brace = imp
        + before[imp..]
            .find('{')
            .expect("import 花括号应在 from 之前");
    let block = before[brace + 1..]
        .split('}')
        .next()
        .expect("花括号闭合在 from 之前");
    block
        .split(',')
        .map(|s| s.trim())
        .map(|s| s.strip_prefix("type ").map(str::trim).unwrap_or(s))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// 模块是否导出了这个名字（`export [async] <kw> <name>` + 词边界）。
fn is_exported(src: &str, name: &str) -> bool {
    [
        "function",
        "async function",
        "const",
        "interface",
        "type",
        "class",
    ]
    .into_iter()
    .any(|kw| {
        let pat = format!("export {kw} {name}");
        src.match_indices(&pat).any(|(i, _)| {
            src[i + pat.len()..]
                .chars()
                .next()
                .map_or(true, |c| !c.is_ascii_alphanumeric() && c != '_')
        })
    })
}

/// spec 的 `test('…')` 题面集合（场景 8 第 6 步的同题比对用）。
fn test_titles(spec: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for line in spec.lines() {
        let t = line.trim_start();
        let Some((rest, quote)) = t
            .strip_prefix("test('")
            .map(|r| (r, '\''))
            .or_else(|| t.strip_prefix("test(\"").map(|r| (r, '"')))
        else {
            continue;
        };
        if let Some(end) = rest.find(quote) {
            set.insert(rest[..end].to_string());
        }
    }
    set
}

/// INDEX 候选表的一行（6 列：编号 | 标题 | 预计证据类型 | 与前轮关联 | 证据等级 | 状态）。
struct IndexRow {
    nn: String,
    file: String,
    cells: Vec<String>,
}

fn index_rows(index: &str) -> Vec<IndexRow> {
    let mut rows = Vec::new();
    for line in index.lines() {
        let t = line.trim();
        if !(t.starts_with("| [") && t.contains("](")) {
            continue;
        }
        let cells: Vec<String> = t
            .trim_matches('|')
            .split(" | ")
            .map(|c| c.trim().to_string())
            .collect();
        let first = &cells[0];
        let bytes = first.as_bytes();
        assert!(
            bytes.len() > 3 && bytes[0] == b'[' && bytes[3] == b']',
            "候选表编号列形如 [NN](file)：{first}"
        );
        let nn = format!("{}{}", bytes[1] as char, bytes[2] as char);
        let file = first
            .split("](")
            .nth(1)
            .and_then(|s| s.split(')').next())
            .expect("编号列应带文件链接")
            .to_string();
        rows.push(IndexRow { nn, file, cells });
    }
    rows
}

fn bodies_texts(root: &Path) -> Vec<(String, String)> {
    body_rels(root)
        .into_iter()
        .map(|rel| {
            let txt = read(root, &rel);
            (rel, txt)
        })
        .collect()
}

/// README「## 五」节里点名的票号（与票面 `未验证` 双向一致用）。
fn readme_unverified_tickets(readme: &str) -> BTreeSet<String> {
    let start = readme.find("## 五").expect("README 应有「五、未验证」节");
    let end = readme[start..]
        .find("## 六")
        .map(|e| start + e)
        .unwrap_or(readme.len());
    let bytes = readme.as_bytes();
    let sec = &bytes[start..end];
    let mut set = BTreeSet::new();
    let mut from = 0;
    while let Some(i) = sec[from..].iter().position(|&b| b == b'/') {
        let at = from + i + 1;
        // 「issues/NN-」的 NN——只要紧邻 issues/ 的两个字节是数字。
        if at + 1 < sec.len() && sec[at].is_ascii_digit() && sec[at + 1].is_ascii_digit() {
            let _ = sec[at - 1] == b's'; // 形状提示：位置紧贴「issues/」
            set.insert(format!("{}{}", sec[at] as char, sec[at + 1] as char));
        }
        from = at + 1;
    }
    set
}

fn line_n(path: &Path, n: usize) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("读 {path:?} 失败：{e}"))
        .lines()
        .nth(n - 1)
        .unwrap_or_else(|| panic!("{path:?} 没有第 {n} 行"))
        .to_string()
}

/// 在 frontend/src 下按文件名找一个源码文件（出处点名的行号核对用）。
fn find_named(root: &Path, name: &str) -> PathBuf {
    fn walk(dir: &Path, name: &str) -> Option<PathBuf> {
        for entry in std::fs::read_dir(dir).ok()? {
            let p = entry.ok()?.path();
            if p.is_dir() {
                if let Some(hit) = walk(&p, name) {
                    return Some(hit);
                }
            } else if p.file_name().is_some_and(|f| f == name) {
                return Some(p);
            }
        }
        None
    }
    walk(&root.join("frontend/src"), name).unwrap_or_else(|| panic!("frontend/src 下应有 {name}"))
}

// ─────────────────────────── 场景 1（AC-1） ───────────────────────────

/// 骨架先落盘：INDEX 与占位壳同落、占位可证、回填在更晚的提交、正文不早于骨架。
#[test]
fn scene_01_index_skeleton_lands_with_placeholders_before_backfill() {
    let root = root();
    assert!(
        root.join(INDEX_REL).exists(),
        "00-INDEX.md 必须存在（骨架本体）"
    );
    if !history_available(&root) {
        eprintln!(
            "场景 1：历史不足（depth=1 checkout）——git 顺序子断言跳过，\
             完整证据见 test-report.md 命令台账"
        );
        return;
    }
    let first = git(
        &root,
        &["log", "--diff-filter=A", "--format=%H %ct", "--", INDEX_REL],
    )
    .filter(|s| !s.trim().is_empty())
    .expect("INDEX 应有首次入库记录");
    let mut parts = first.split_whitespace();
    let idx_hash = parts.next().expect("骨架提交 hash");
    let idx_ct = parts.next().expect("骨架提交 ct");
    let idx_ct: i64 = idx_ct.parse().expect("commit time 是整数");

    // 骨架提交里 INDEX 与 13 个正文文件一起可见。
    let tree = git(
        &root,
        &["ls-tree", "-r", "--name-only", idx_hash, "--", ISSUES_DIR],
    )
    .expect("ls-tree 骨架提交");
    let rels = body_rels(&root);
    assert_eq!(rels.len(), 13, "正文恰 13 条");
    for rel in rels.iter().chain(std::iter::once(&INDEX_REL.to_string())) {
        assert!(tree.lines().any(|l| l.trim() == rel), "骨架提交缺 {rel}");
    }

    // 骨架版 INDEX 的证据列是「待填」占位（先于核实阶段落盘的直接证据）。
    let skeleton_index =
        git(&root, &["show", &format!("{idx_hash}:{INDEX_REL}")]).expect("读骨架版 INDEX");
    assert!(
        skeleton_index.matches("待填").count() >= 13,
        "骨架版 INDEX 应含 ≥13 处「待填」占位，实为 {}",
        skeleton_index.matches("待填").count()
    );

    for rel in &rels {
        let first_add = git(
            &root,
            &["log", "--diff-filter=A", "--format=%H %ct", "--", rel],
        )
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| panic!("{rel} 应有首次入库记录"));
        let mut parts = first_add.split_whitespace();
        let hash = parts.next().expect("正文首次入库 hash");
        let ct = parts.next().expect("正文首次入库 ct");
        let ct: i64 = ct.parse().expect("commit time 是整数");
        // 判失败条件只有一条：正文严格早于骨架（场景 1 预期 3）。同秒同刻
        // = 骨架与空壳原子落盘，按设计 AC-1 的「补齐之前已可见」口径放行（文件头说明 2）。
        assert!(
            ct >= idx_ct,
            "{rel} 的首次入库（{ct}）早于骨架 INDEX（{idx_ct}）——先正文后骨架正是上一轮烂尾机制"
        );
        if ct == idx_ct {
            eprintln!("场景 1 口径注：{rel} 与骨架同秒入库（空壳原子落盘，占位→回填链可证）");
        }
        // 骨架版是「待填」空壳；现版已回填、且至少两次提交（骨架 + 回填）。
        let skeleton = git(&root, &["show", &format!("{hash}:{rel}")])
            .unwrap_or_else(|| panic!("读骨架版 {rel}"));
        assert!(
            skeleton.contains("待填"),
            "{rel} 的骨架版应是「待填」占位壳"
        );
        let current = read(&root, rel);
        assert!(!current.contains("待填"), "{rel} 现版仍有占位未回填");
        let commits = git(&root, &["log", "--format=%H", "--", rel])
            .expect("log 正文")
            .split_whitespace()
            .count();
        assert!(
            commits >= 2,
            "{rel} 只见 {commits} 次提交——回填发生在更晚的提交才可证"
        );
    }
}

// ─────────────────────────── 场景 2（AC-2） ───────────────────────────

/// 候选编号集合与 NN-slug 文件集合相等，每条正文四栏齐备且无占位标记。
#[test]
fn scene_02_every_candidate_has_a_body_with_four_fields() {
    let root = root();
    let index = read(&root, INDEX_REL);
    let rows = index_rows(&index);
    let rels = body_rels(&root);

    let idx_nn: BTreeSet<String> = rows.iter().map(|r| r.nn.clone()).collect();
    let file_nn: BTreeSet<String> = rels
        .iter()
        .map(|r| r.rsplit('/').next().unwrap()[..2].to_string())
        .collect();
    let missing: Vec<_> = idx_nn.difference(&file_nn).collect();
    let extra: Vec<_> = file_nn.difference(&idx_nn).collect();
    println!(
        "candidates={}, bodies={}, missing={:?}, extra={:?}",
        idx_nn.len(),
        file_nn.len(),
        missing,
        extra
    );
    assert_eq!(idx_nn.len(), 13, "候选表恰 13 行");
    assert_eq!(file_nn.len(), 13, "正文恰 13 份");
    assert!(
        missing.is_empty() && extra.is_empty(),
        "有行无票 / 有票无行：missing={missing:?} extra={extra:?}"
    );
    // 链接列与文件名一一对应（编号前缀 ↔ 链接目标）。
    for row in &rows {
        assert_eq!(
            row.file,
            rels.iter()
                .find(|r| r.ends_with(&format!("/{}", row.file)))
                .map(|_| row.file.clone())
                .unwrap_or_else(|| panic!("候选表链接的 {} 不存在", row.file)),
            "候选 {} 的链接应指向存在的文件",
            row.nn
        );
        assert!(
            row.file.starts_with(&format!("{}-", row.nn)),
            "编号与文件前缀不一致：{} ↔ {}",
            row.nn,
            row.file
        );
    }

    for (rel, txt) in bodies_texts(&root) {
        for label in ["出处", "严重度", "证据等级"] {
            assert!(
                field(&txt, label).is_some_and(|v| !v.is_empty()),
                "{rel} 缺非空的「{label}」栏"
            );
        }
        assert!(
            rel_field(&txt).is_some_and(|v| !v.is_empty()),
            "{rel} 缺非空的「与前轮关联（=与前轮的关系）」栏"
        );
        for placeholder in ["待填", "TODO", "<取什么>"] {
            assert!(
                !txt.contains(placeholder),
                "{rel} 含未填标记「{placeholder}」——正文不是空壳（决策 376④②）"
            );
        }
    }
}

// ─────────────────────────── 场景 3（AC-3） ───────────────────────────

/// 票 18/21/22 三条各有结论（落四档）+ 判据，且与源码现状逐条对得上。
#[test]
fn scene_03_three_named_rechecks_have_conclusions_and_criteria() {
    let root = root();
    let bodies = bodies_texts(&root);

    for prior in ["18", "21", "22"] {
        let hits: Vec<&(String, String)> = bodies
            .iter()
            .filter(|(_, t)| rel_field(t).is_some_and(|v| v.contains(&format!("=前轮 {prior}"))))
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "前轮票 {prior} 应恰对应一条正文，实为 {:?}",
            { hits.iter().map(|(r, _)| r.as_str()).collect::<Vec<_>>() }
        );
        let (rel, txt) = hits[0];
        let bucket = conclusion_bucket(txt).unwrap_or_else(|| panic!("{rel} 没有「结论：」行"));
        assert!(
            matches!(bucket, "已修" | "未修" | "回归" | "有意不做"),
            "{rel} 结论档「{bucket}」不在四档之内"
        );
        assert_eq!(
            bucket, "有意不做",
            "{rel}（=前轮 {prior}，wontfix）须按设计 §4 特别规则落「有意不做」而非「未修」"
        );
        assert!(
            has_source_ref(txt) || txt.contains("[r3]"),
            "{rel} 缺判据（源码 行号 或 实测读数）"
        );
        if prior == "22" {
            assert!(txt.contains("partial"), "{rel} 未写明 status=partial");
            // 已落两件（router 底座 + 班次）与未落三件分开记。
            assert!(
                txt.contains("router.svelte.ts") && txt.contains("talkSessions.ts"),
                "{rel} 应记已落的两件底座（router + talkSessions）"
            );
            for unlanded in ["?tab=", "?filter=", "talk_draft"] {
                assert!(
                    txt.contains(unlanded),
                    "{rel} 应记未落的三件之一：{unlanded}"
                );
            }
        }
        // 出处栏应点名评审订正后的支撑行（hero 出处 = PipelineRail）。
        if prior == "18" {
            let out = field(txt, "出处").expect("01 应有出处栏");
            assert!(
                out.contains("PipelineRail.svelte:340-344"),
                "01 的 hero 出处应为 PipelineRail.svelte:340-344（评审订正项），实为：{out}"
            );
        }
        if prior == "21" {
            let out = field(txt, "出处").expect("03 应有出处栏");
            assert!(
                out.contains("PendingActions.svelte:115,131,143"),
                "03 的量级出处应为 PendingActions.svelte:115,131,143（评审订正项），实为：{out}"
            );
            assert!(!txt.contains("163-172"), "03 不应再引用已证伪的 :163-172");
        }
    }

    // ── 第 5 步：交叉核对源码现状 ──
    //
    // 票面（`.scratch/ux-audit-3/issues/*`）是**冻结的审计当轮记录**，仍写「未落 / 有意不做」，
    // 一字不改（见 landing::scene_15 的 FROZEN）。2026-10-01 用户裁决把原 wontfix 的票 01 /
    // 票 04 落地之后，这里核的是**源码现状 = 已落**；落地事实另记 `.scratch/ux-audit-3/IMPLEMENTATION.md`
    // 的票 01 / 票 04 两节。票 21（票 03 破坏性确认步）本轮**未落**，源码侧照旧是「没有」——
    // 那一半的牙齿留在下面，等它落地时再翻。
    let task_detail = read(&root, "frontend/src/routes/TaskDetail.svelte");
    assert!(
        line_n(&root.join("frontend/src/routes/TaskDetail.svelte"), 48)
            .contains("let tab = $state"),
        "TaskDetail.svelte:48 应是 `let tab = $state<Tab>(tabFromQuery(…))`（?tab= 已接地址）"
    );
    assert!(
        task_detail.contains("readQuery") && task_detail.contains("writeQuery"),
        "详情页应从 router 读写 ?tab=（决策 217①③，2026-10-01 落地）"
    );
    assert!(
        line_n(&root.join("frontend/src/stores/board.svelte.ts"), 87)
            .contains("filter = $state<StatusFilter>"),
        "board.svelte.ts:87 应是按「地址 → 本地兜底 → 缺省」初始化的 filter 态"
    );
    let board_route = read(&root, "frontend/src/routes/Board.svelte");
    assert!(
        board_route.contains("syncFilterFromQuery") && board_route.contains("r.query.filter"),
        "看板路由应把 ?filter= 交给 store 恢复（决策 217④，2026-10-01 落地）"
    );
    let board_store = read(&root, "frontend/src/stores/board.svelte.ts");
    assert!(
        board_store.contains("writeQuery({ filter:") && board_store.contains("readQuery().filter"),
        "看板 store 应把 ?filter= 从地址读、往地址写（决策 217③④）"
    );
    assert!(
        line_n(&root.join("frontend/src/routes/Talk.svelte"), 1090).contains("let qdraft = $state"),
        "Talk.svelte:1090 应是排队条编辑草稿态（主输入草稿另走 lib/talkDraft.ts）"
    );
    let talk_draft_hits: Vec<String> = walk_source_hits(&root, "talk_draft");
    assert!(
        talk_draft_hits
            .iter()
            .any(|p| p.ends_with("lib/talkDraft.ts")),
        "全仓应有 talkDraft 落点（agentpipeline.talk_draft，决策 217⑤）：{talk_draft_hits:?}"
    );
    assert!(
        read(&root, "frontend/src/lib/talkDraft.test.ts").contains("TALK_DRAFT_MAX_AGE_MS"),
        "talkDraft.test.ts 应钉 7 天过期（决策 217⑤ 的单测那一格）"
    );
    // 2026-10-01 用户裁决推翻 wontfix 之后，票 21（票 03 破坏性确认步）随本轮落地：
    // `actionTier` 三档量级在源码里，且审批两处（PendingActions / DiffReviewPanel）都接了
    // 确认句——这一半的牙齿从「不该有」翻成「必须有」。
    let tier_hits: Vec<String> = walk_source_hits(&root, "actionTier");
    assert!(
        tier_hits.iter().any(|p| p.ends_with("lib/actions.ts")),
        "actionTier 应落在 lib/actions.ts（票 21 / 票 03，2026-10-01 落地）：{tier_hits:?}"
    );
    for wired in [
        "frontend/src/components/board/PendingActions.svelte",
        "frontend/src/components/task/DiffReviewPanel.svelte",
    ] {
        let src = read(&root, wired);
        assert!(
            src.contains("actionTier") && src.contains("confirmSentence"),
            "{wired} 应接两步确认（量级 + 后果句）"
        );
    }
    let router = read(&root, "frontend/src/router.svelte.ts");
    let router_lines: Vec<&str> = router.lines().collect();
    assert!(
        router_lines[179].contains("readQuery"),
        "router.svelte.ts:180 应是 readQuery 底座"
    );
    assert!(
        router_lines[195].contains("writeQuery"),
        "router.svelte.ts:196 应是 writeQuery 底座"
    );
    assert!(
        read(&root, "frontend/src/router.test.ts").contains("readQuery"),
        "router.test.ts 应有用例覆盖 readQuery"
    );
    assert!(
        line_n(&root.join("frontend/src/lib/talkSessions.ts"), 21).contains("TALK_SESSION_KEY")
            && line_n(&root.join("frontend/src/lib/talkSessions.ts"), 21)
                .contains("agentpipeline.talk_session"),
        "talkSessions.ts:21 应是班次键常量"
    );
}

/// 在 frontend/src 的文本文件里搜一个串（场景 3 的反证扫描）。
fn walk_source_hits(root: &Path, needle: &str) -> Vec<String> {
    let mut hits = Vec::new();
    fn walk(dir: &Path, needle: &str, hits: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, needle, hits);
            } else if p
                .extension()
                .is_some_and(|e| matches!(e.to_str(), Some("ts" | "svelte" | "css" | "js")))
                && std::fs::read_to_string(&p).is_ok_and(|t| t.contains(needle))
            {
                hits.push(p.display().to_string());
            }
        }
    }
    walk(&root.join("frontend/src"), needle, &mut hits);
    hits
}

// ─────────────────────────── 场景 4（AC-5） ───────────────────────────

/// 证据等级 ∈ {实测, 代码, 未验证} 且与出处形态一致；抽查的行号 100% 命中。
#[test]
fn scene_04_evidence_levels_are_traceable_with_source_spot_checks() {
    let root = root();
    for (rel, txt) in bodies_texts(&root) {
        let level = field(&txt, "证据等级").unwrap_or_else(|| panic!("{rel} 缺「证据等级」栏"));
        let tiers = ["实测", "代码", "未验证"]
            .into_iter()
            .filter(|t| level.contains(t))
            .collect::<Vec<_>>();
        assert!(!tiers.is_empty(), "{rel} 证据等级「{level}」不落三档");
        if level.contains("实测") {
            assert!(
                txt.contains("[r3]") || !cited_pngs(&txt).is_empty(),
                "{rel} 标「实测」却既无可引用读数也无截图名——不许把读源码可推写成实测（R4）"
            );
        }
        if !level.contains("实测") {
            assert!(
                has_source_ref(&txt),
                "{rel} 标「代码」级却无 路径:行号 出处"
            );
        }
        if level.contains("未验证") {
            let why = [
                "harness",
                "未在真页面",
                "只走了看板路由",
                "跑不动",
                "只在源码层",
                "留待",
                "没弹出",
                "该态下无此动作",
                "全文件 0 命中",
            ];
            assert!(
                why.iter().any(|k| txt.contains(k)),
                "{rel} 标「未验证」却没写明为什么跑不动"
            );
        }
    }

    // 抽查 ≥3 条出处行号（覆盖 01/03/04 点名的行；行号内容与正文描述对得上）。
    // 2026-10-01 票 01 / 票 04 落地后行号随迁：TaskDetail 的 `let tab` 40 → 48（tab 块 +8）、
    // board 的 `filter = $state` 37 → 87（地址/本地兜底那组函数 +50）。
    let spot: [(&str, usize, &str); 6] = [
        ("TaskDetail.svelte", 48, "let tab = $state"),
        ("PipelineRail.svelte", 340, ".rail.hero"),
        ("board.svelte.ts", 87, "filter = $state<StatusFilter>"),
        ("router.svelte.ts", 180, "readQuery"),
        ("talkSessions.ts", 21, "TALK_SESSION_KEY"),
        ("PendingActions.svelte", 86, "btn solid"),
    ];
    for (name, n, expect) in spot {
        let path = find_named(&root, name);
        let line = line_n(&path, n);
        assert!(
            line.contains(expect),
            "{}:{n} 应含「{expect}」，实为：{}",
            path.display(),
            line.trim()
        );
    }
    // PipelineRail 340-350 的 hero 段**有** overflow-x: auto（票 01 已于 2026-10-01 落地：
    // 容器内横滚，不裁切、不传文档——原「hero 默认 visible」那句论断随 wontfix 推翻作废）。
    let rail = find_named(&root, "PipelineRail.svelte");
    let rail_txt = std::fs::read_to_string(&rail).expect("读 PipelineRail.svelte");
    let rail_lines: Vec<&str> = rail_txt.lines().collect();
    let hero_band = rail_lines[339..350].join("\n");
    assert!(
        hero_band.contains("overflow-x: auto"),
        "PipelineRail.svelte:340-350 应含 overflow-x: auto（票 01 落地的容器内横滚）"
    );
    // PendingActions 的量级两档支撑行（票 03 落地后随接线随迁：量级 88、确认态取消钮 197；
    // 2026-10-08 确认态判据由「数组身份」改为「动作身份串」时取消钮再 +9 → 206）。
    let pending = find_named(&root, "PendingActions.svelte");
    assert!(
        line_n(&pending, 88).contains("btn quiet"),
        ":88 应是 quiet 档"
    );
    assert!(
        line_n(&pending, 206).contains("btn quiet"),
        ":206 应是确认态取消钮（quiet）"
    );
}

// ─────────────────────────── 场景 5（AC-6） ───────────────────────────

/// 「与前轮关联」取值合法、点名的前轮票号全部可达、wontfix 不复活、决策 215/216/217 未动。
#[test]
fn scene_05_prior_round_links_resolve_and_wontfix_stays_closed() {
    let root = root();
    let prior_dir2 = root.join(".scratch/ux-audit-2/issues");
    let mut opened = 0usize;

    for (rel, txt) in bodies_texts(&root) {
        let value = rel_field(&txt).unwrap_or_else(|| panic!("{rel} 缺「与前轮关联」栏"));
        assert!(
            value.starts_with("新开")
                || value.starts_with("重复")
                || value.starts_with("回归")
                || value.starts_with("现状核实"),
            "{rel} 关联值「{value}」不落既定形态（新开/重复/回归/wontfix 项的现状核实）"
        );
        if value.starts_with("新开") {
            opened += 1;
        }
        // 「=前轮 NN」点名的票必须真实存在（wontfix 映射）。
        let mut from = 0;
        while let Some(i) = txt[from..].find("=前轮 ") {
            let at = from + i + "=前轮 ".len();
            let digits: String = txt[at..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            assert!(!digits.is_empty(), "{rel} 的「=前轮」后应跟票号");
            let exists = std::fs::read_dir(&prior_dir2)
                .expect("ux-audit-2/issues 应存在")
                .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
                .any(|f| f.starts_with(&format!("{digits}-")));
            assert!(
                exists,
                "{rel} 点名的前轮票 {digits} 在 .scratch/ux-audit-2/issues/ 不存在（悬空引用）"
            );
            from = at + digits.len();
        }
        // 相对链接可达（场景 5 第 2 步）：锚点二选——票文件目录 / 轮次根
        // （见文件头口径 4）。被点名的票文件存在性另由上面 `=前轮 NN` 硬证。
        for link in prior_links(&txt) {
            let from_file = root.join(ISSUES_DIR).join(&link);
            let from_round = root.join(".scratch/ux-audit-3").join(&link);
            assert!(
                from_file.exists() || from_round.exists(),
                "{rel} 的链接在票目录与轮次根两个锚下都悬空：{link}"
            );
        }
    }
    assert!(opened >= 1, "应有漂移面新开条目");

    // wontfix 专项：票 18/21/22 只记「现状核实」、结论「有意不做」，前轮 Status 对得上。
    for (nn, expect_status) in [
        ("18", "wontfix"),
        ("19", "superseded"),
        ("21", "wontfix"),
        ("22", "wontfix"),
    ] {
        let prior_file = std::fs::read_dir(&prior_dir2)
            .expect("ux-audit-2/issues 应存在")
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
            .find(|f| f.starts_with(&format!("{nn}-")))
            .unwrap_or_else(|| panic!("前轮票 {nn} 不存在"));
        let txt = read(&root, &format!(".scratch/ux-audit-2/issues/{prior_file}"));
        assert!(
            txt.contains(expect_status),
            "前轮票 {nn} 的 Status 应为 {expect_status}"
        );
    }
    for (rel, txt) in bodies_texts(&root) {
        let value = rel_field(&txt).unwrap_or_default();
        for closed in ["=前轮 18", "=前轮 21"] {
            if value.contains(closed) {
                assert!(
                    value.starts_with("现状核实"),
                    "{rel} 把已 wontfix 的 {closed} 以「{value}」姿态复活（R3 防线）"
                );
                assert_eq!(
                    conclusion_bucket(&txt),
                    Some("有意不做"),
                    "{rel} 对 wontfix 票的结论须是「有意不做」"
                );
            }
        }
    }

    // 决策 215/216/217 是既成裁决：正文存在（分支无关判据，照跑）。
    // 「本轮一字未动」那半句按 `{merge-base}...HEAD` 取数——那是**审计那一轮**的分支 diff，
    // 已摘到 `#[ignore]` 的 `artifact_shape_readings_once`（按分支取数不是闸门判据，见
    // `docs/testing.md` §8）。
    let decisions = read(&root, "docs/decisions.md");
    for dn in ["决策 215", "决策 216", "决策 217"] {
        assert!(decisions.contains(dn), "decisions.md 应有 {dn}");
    }

    // 漂移面来源核对（design §1-B 点名的新开面逐条在册）。
    let anchors: [(&str, &str); 7] = [
        ("07", "SettingsTools"),
        ("08", "SettingsNotify"),
        ("09", "SettingsForeman"),
        ("10", "道具栏"),
        ("11", "决策 240"),
        ("12", "决策 243"),
        ("13", "决策 215"),
    ];
    for (nn, anchor) in anchors {
        let (rel, txt) = bodies_texts(&root)
            .into_iter()
            .find(|(r, _)| {
                r.rsplit('/')
                    .next()
                    .is_some_and(|name| name.starts_with(nn))
            })
            .unwrap_or_else(|| panic!("票 {nn} 缺失"));
        assert!(
            rel_field(&txt).is_some_and(|v| v.starts_with("新开")),
            "{rel} 应是漂移面新开"
        );
        assert!(txt.contains(anchor), "{rel} 应点名漂移面锚「{anchor}」");
    }
}

// ─────────────────────────── 场景 6（AC-7） ───────────────────────────

/// 产品代码零 diff；分支变更只落白名单（+ 本阶段测试文件）；.gitignore 恰 +注释+一行 glob。
///
/// **`#[ignore]`（一次性落地形状验收）**：判据是「审计那一轮的**分支 diff**」——一落 main
/// 就再也取不到（`origin/main` 上已不存在任何区间能重现它，见 `docs/testing.md` §8）。原先
/// 靠三道守卫（shallow / 同点 / 审计产物已在基准树）把它挡在自动门外，仍会在**别的**任务
/// 分支上假红。重跑：`cargo test -p e2e --test integration scene_06_ -- --ignored`。
#[test]
#[ignore = "一次性落地形状验收：锚在审计那一轮的分支 diff 上，不是分支无关的回归判据"]
fn scene_06_product_code_zero_diff_and_sole_allowed_changes() {
    let root = root();
    let mb = merge_base(&root).expect("merge-base HEAD origin/main 应可得（本用例需完整历史）");
    let range = format!("{mb}...HEAD");

    // AC-7 点名口径：frontend/src 与 crates 字面零 diff。
    let product = git(
        &root,
        &["diff", "--stat", &range, "--", "frontend/src", "crates"],
    )
    .expect("diff 产品代码");
    assert!(
        product.trim().is_empty(),
        "产品代码出现 diff（AC-7：审计落票不修，不直接改代码）：\n{product}"
    );

    // 全量变更白名单：审计产出三处 + 本阶段测试产出（文件头说明 3）。
    let names = git(&root, &["diff", "--name-only", &range]).expect("diff name-only");
    let allowed = |p: &str| {
        p == ".gitignore"
            || p.starts_with(".scratch/ux-audit-3/")
            || p == "frontend/e2e/ux-audit-3.spec.ts"
            || p.starts_with("tests/e2e/tests/integration/")
    };
    let foreign: Vec<&str> = names
        .lines()
        .filter(|l| !l.is_empty() && !allowed(l))
        .collect();
    assert!(
        foreign.is_empty(),
        "白名单外的变更（场景 6 第 3 步）：{foreign:?}"
    );
    assert!(
        names.lines().any(|l| l == SPEC_REL),
        "新 spec 应在本轮变更里"
    );

    // .gitignore：无删除、恰一行注释 + 一行 glob（+2 行形态照 :25-28 先例；
    // 场景原文「恰好 +1 行」与其括注自相矛盾，评审口径按注释+glob 计，见报告）。
    let gi_diff = git(&root, &["diff", &range, "--", ".gitignore"]).expect("diff .gitignore");
    let mut adds = Vec::new();
    let mut dels = Vec::new();
    for line in gi_diff.lines() {
        // 跳过 `--- a/…` / `+++ b/…` 文件头（它们以 `+`/`-` 开头但不是内容行）。
        if line.starts_with("+++") || line.starts_with("--- ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix('+') {
            adds.push(rest.to_string());
        } else if let Some(rest) = line.strip_prefix('-') {
            dels.push(rest.to_string());
        }
    }
    assert!(dels.is_empty(), ".gitignore 不应有删除：{dels:?}");
    let globs: Vec<&str> = adds
        .iter()
        .map(|a| a.trim_start())
        .filter(|a| a.starts_with(".scratch/"))
        .collect();
    let comments: Vec<&str> = adds
        .iter()
        .map(|a| a.trim_start())
        .filter(|a| a.starts_with('#'))
        .collect();
    assert_eq!(
        globs,
        vec![".scratch/ux-audit-3/*.png"],
        ".gitignore 恰加一行 ux-audit-3 的 PNG glob"
    );
    assert_eq!(comments.len(), 1, "恰一行注释：{comments:?}");
    assert!(
        comments[0].contains("UX_AUDIT3")
            && comments[0].contains("重新生成")
            && comments[0].contains("不入库"),
        "注释形态照先例（UX_AUDIT3 + 可重新生成 + 不入库）：{}",
        comments[0]
    );
    assert_eq!(
        adds.len(),
        2,
        ".gitignore 恰 +2 行（注释 + glob），实为：{adds:?}"
    );
}

// ─────────────────────────── 场景 7（AC-4） ───────────────────────────

/// 新 spec 存在、文件级 skip 守卫在首位、14 例、只 import 既有 harness/scripts 导出。
#[test]
fn scene_07_new_spec_exists_and_skips_by_default() {
    let root = root();
    let spec = read(&root, SPEC_REL);
    let guard = "test.skip(process.env.UX_AUDIT3 !== '1'";
    assert!(
        spec.contains(guard),
        "spec 应有文件级 skip 守卫（形状照 ux-audit-2.spec.ts:33）：{guard}"
    );
    let guard_pos = spec.find(guard).expect("守卫位置");
    // 逐行扫描找首条用例与用例计数（用例可能有缩进，见场景 7 第 3 步）。
    let mut offset = 0usize;
    let mut first_test_pos: Option<usize> = None;
    let mut tcount = 0usize;
    for line in spec.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with("test(") {
            tcount += 1;
            first_test_pos.get_or_insert(offset + (line.len() - trimmed.len()));
        }
        offset += line.len();
    }
    let first_test_pos = first_test_pos.expect("spec 应至少有一条 test(");
    assert!(
        guard_pos < first_test_pos,
        "skip 守卫必须在全部用例之前（文件级跳过）"
    );
    assert_eq!(tcount, 14, "收集 14 例（与 README 记载一致）");
    let audit_readme = read(&root, AUDIT_README_REL);
    assert!(
        audit_readme.contains("14 例"),
        "README 应记默认 14 例全跳的口径"
    );

    // 零新可测试性接缝（决策 143 五条不动）：import 的每个名字都是既有模块的导出。
    for name in imported_from(&spec, "harness") {
        let harness = read(&root, "frontend/e2e/harness.ts");
        assert!(
            is_exported(&harness, &name),
            "harness.ts 未导出 {name}——spec 不得新造接缝"
        );
    }
    for name in imported_from(&spec, "scripts") {
        let scripts = read(&root, "frontend/e2e/scripts.ts");
        assert!(
            is_exported(&scripts, &name),
            "scripts.ts 未导出 {name}——spec 不得新造接缝"
        );
    }
}

// ─────────────────────────── 场景 8（AC-4，静态前半） ───────────────────────────

/// stdout 有可引用读数（[r3] 打印口）、截图名与落盘 PNG 双向一致（PNG 存在时）。
#[test]
fn scene_08_readings_and_screenshots_are_citable() {
    let root = root();
    let spec = read(&root, SPEC_REL);
    assert!(
        spec.contains("console.log") && spec.contains("[r3]"),
        "spec 应把可引用读数打到 stdout（[r3] 打印口）"
    );
    assert!(
        spec.contains("ux-audit-3") && spec.contains(".png"),
        "spec 应把截图落到 .scratch/ux-audit-3/"
    );

    // 场景 8 第 6 步（静态面）：取证范围是「复核 + 漂移面」——用例题面与前两轮
    // 不同题（题面全名不撞）。live 跑出的用例名清单见报告台账。
    let own = test_titles(&spec);
    assert_eq!(own.len(), 14, "题面抽取应得 14 条");
    for prior in [
        "frontend/e2e/ux-audit.spec.ts",
        "frontend/e2e/ux-audit-2.spec.ts",
    ] {
        let prior_titles = test_titles(&read(&root, prior));
        let dup: Vec<&String> = own.intersection(&prior_titles).collect();
        assert!(
            dup.is_empty(),
            "与 {prior} 同题重复取证（场景 8 第 6 步）：{dup:?}"
        );
    }

    // 引用的截图名 ⊆ 真实落盘（有 PNG 才断言——PNG 是 gitignore 的运行产物，
    // CI checkout 里不存在；本机跑过 UX_AUDIT3=1 后由场景 8 台账佐证）。
    // 方向按场景 8 原文：「被引用的截图名 ⊆ 真实落盘 PNG 名」；
    // 落盘未被引用的多余文件只记台账不判失败（可由早前的取证运行遗留）。
    let dir = root.join(".scratch/ux-audit-3");
    let actual: BTreeSet<String> = std::fs::read_dir(&dir)
        .map(|it| {
            it.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    if actual.is_empty() {
        eprintln!(
            "场景 8：目录内暂无 PNG（未跑 UX_AUDIT3=1 或 CI checkout）——截图存在性由报告台账承担"
        );
        return;
    }
    let mut cited = BTreeSet::new();
    let audit_readme = read(&root, AUDIT_README_REL);
    cited.extend(cited_pngs(&audit_readme));
    for (rel, txt) in bodies_texts(&root) {
        let pngs = cited_pngs(&txt);
        if (field(&txt, "证据等级").unwrap_or_default()).contains("实测") {
            assert!(
                txt.contains("[r3]") || !pngs.is_empty(),
                "{rel} 标「实测」既无 [r3] 读数也无截图名"
            );
        }
        cited.extend(pngs);
    }
    let missing: Vec<&String> = cited.difference(&actual).collect();
    assert!(
        missing.is_empty(),
        "被引用的截图名没有真实落盘（虚报面）：{missing:?}"
    );
    let uncited: Vec<&String> = actual.difference(&cited).collect();
    eprintln!(
        "场景 8：引用截图 {} 张、落盘 {} 张、落盘未引用={uncited:?}（后者仅台账）",
        cited.len(),
        actual.len()
    );
}

// ─────────────────────────── 场景 9（AC-3/AC-7） ───────────────────────────

/// 前两轮 spec 文件本轮零 diff，且仍然在位（复跑属台账证据）。
#[test]
fn scene_09_prior_audit_specs_untouched() {
    let root = root();
    for spec in [
        "frontend/e2e/ux-audit.spec.ts",
        "frontend/e2e/ux-audit-2.spec.ts",
    ] {
        assert!(root.join(spec).exists(), "{spec} 应在位");
    }
    // 「前两轮 spec 本轮零 diff」是**审计那一轮**的分支 diff 判据，已摘到 `#[ignore]` 的
    // `artifact_shape_readings_once`（按分支取数不是闸门判据，见 `docs/testing.md` §8）。
}

// ─────────────────────────── 场景 10（AC-8） ───────────────────────────

/// README 四条复现命令齐备、命令指向的文件真实存在、「未验证」清单与票面双向一致。
#[test]
fn scene_10_readme_lists_reproducible_commands_and_unverified_items() {
    let root = root();
    let readme = read(&root, AUDIT_README_REL);
    assert!(
        readme.contains("bash scripts/e2e-artifacts.sh"),
        "README 缺产物构建命令"
    );
    for (env, spec) in [
        ("UX_AUDIT=1", "ux-audit.spec.ts"),
        ("UX_AUDIT2=1", "ux-audit-2.spec.ts"),
        ("UX_AUDIT3=1", "ux-audit-3.spec.ts"),
    ] {
        assert!(readme.contains(env), "README 缺 {env} 复跑命令");
        assert!(readme.contains(spec), "README 缺 {spec} 复跑命令");
    }
    assert!(
        readme.contains("差在哪"),
        "README 应有「本轮与第一/二轮差在哪」一节（口径照第二轮的「〇」节）"
    );

    // 命令存在性：脚本与三条 spec 真实在位。
    assert!(
        root.join("scripts/e2e-artifacts.sh").exists(),
        "scripts/e2e-artifacts.sh 应存在"
    );
    for spec in [
        "frontend/e2e/ux-audit.spec.ts",
        "frontend/e2e/ux-audit-2.spec.ts",
        SPEC_REL,
    ] {
        assert!(root.join(spec).exists(), "{spec} 应存在");
    }

    // 「未验证」清单 ↔ 票面证据等级 双向一致。
    let readme_set = readme_unverified_tickets(&readme);
    let body_set: BTreeSet<String> = bodies_texts(&root)
        .iter()
        .filter(|(_, t)| t.contains("未验证"))
        .map(|(r, _)| r.rsplit('/').next().unwrap()[..2].to_string())
        .collect();
    assert!(
        readme_set == body_set,
        "README「未验证」清单与票面不一致：readme={readme_set:?} bodies={body_set:?}"
    );
}

// ─────────────────────────── 场景 11（AC-1/AC-2） ───────────────────────────

/// 候选池冻结在 A∪B∪C 三源：逐行可归类、C 组 0 条、编号连续、无全量重扫痕迹。
#[test]
fn scene_11_candidate_pool_frozen_to_three_sources() {
    let root = root();
    let index = read(&root, INDEX_REL);
    let rows = index_rows(&index);
    assert_eq!(rows.len(), 13, "候选表 13 行");
    let (mut a, mut b, mut c) = (0, 0, 0);
    for (i, row) in rows.iter().enumerate() {
        assert_eq!(
            row.nn,
            format!("{:02}", i + 1),
            "编号连续且冻结（第 {} 行是 {}）",
            i + 1,
            row.nn
        );
        let source = row
            .cells
            .get(3)
            .unwrap_or_else(|| panic!("候选 {} 缺「与前轮关联」列", row.nn));
        if source.starts_with("A/") {
            a += 1;
        } else if source.starts_with("B/") {
            b += 1;
        } else if source.starts_with("C/") {
            c += 1;
        } else {
            panic!("候选 {} 不在 A∪B∪C 三源内：{source}", row.nn);
        }
    }
    assert_eq!((a, b, c), (6, 7, 0), "A/B/C 计数（C 组本轮 0 条）");
    assert!(
        index.contains("本轮 0 条"),
        "INDEX 应明记 C 组本轮 0 条（只追加、不回头重扫）"
    );
    // 核实阶段是证实/证伪/补证，不是全量重扫（正文不得出现全量枚举痕迹）。
    for (rel, txt) in bodies_texts(&root) {
        assert!(
            !txt.contains("全量"),
            "{rel} 含「全量」——候选池冻结后不允许全量重扫（R1）"
        );
    }
}

// ─────────────────────────── 场景 12（AC-2） ───────────────────────────

/// 正文七栏齐备、边界/建议非空、叠词表合法、只到「建议」层（无 What to build）。
#[test]
fn scene_12_body_shape_stops_at_suggestion_layer() {
    let root = root();
    for (rel, txt) in bodies_texts(&root) {
        for label in ["叠", "来源", "What to see", "证据等级", "建议", "边界"] {
            assert!(
                field(&txt, label).is_some(),
                "{rel} 缺「{label}」栏（形状照 ux-audit-2/issues 既有票）"
            );
        }
        assert!(
            rel_field(&txt).is_some(),
            "{rel} 缺「与前轮关联/与前轮的关系」栏"
        );
        assert!(
            !txt.contains("What to build"),
            "{rel} 含 What to build 实现清单——审计票只到建议层（AC-7 / R2）"
        );
        let boundary = field(&txt, "边界").unwrap_or_default();
        assert!(!boundary.trim().is_empty(), "{rel}「边界」栏为空");
        let suggestion = field(&txt, "建议").unwrap_or_default();
        assert!(!suggestion.trim().is_empty(), "{rel}「建议」栏为空");
        let stack = field(&txt, "叠").unwrap_or_default();
        assert!(
            stack.starts_with("A（不动规格") || stack.starts_with('B'),
            "{rel}「叠」取值不在词表（A（不动规格）/ B…）：{stack}"
        );
    }
}

// ─────────────────────────── 场景 13（AC-7） ───────────────────────────

/// PNG 被 .gitignore 挡住、Markdown 不被误挡、形态照 :25-28 先例、status 无二进制噪声。
#[test]
fn scene_13_png_ignored_markdown_tracked() {
    let root = root();

    let (hit, out) = git_raw(
        &root,
        &["check-ignore", "-v", ".scratch/ux-audit-3/probe.png"],
    );
    assert!(
        hit && out.contains(".gitignore:") && out.contains("ux-audit-3/*.png"),
        "probe.png 应命中 .gitignore 的 ux-audit-3 glob：rc_ok={hit} out={out}"
    );
    let (hit_readme, out_readme) = git_raw(&root, &["check-ignore", "-v", AUDIT_README_REL]);
    assert!(
        !hit_readme,
        "README.md 不应被 glob 挡住（它们要入库）：{out_readme}"
    );
    let sample_body = format!("{ISSUES_DIR}/01-detail-midband-fold-status.md");
    let (hit_body, out_body) = git_raw(&root, &["check-ignore", "-v", &sample_body]);
    assert!(!hit_body, "票正文不应被挡：{out_body}");

    // .gitignore 形态与先例同规（注释 + glob；先例两条仍在）。
    let gitignore = read(&root, ".gitignore");
    let lines: Vec<&str> = gitignore.lines().collect();
    let pos = lines
        .iter()
        .position(|l| l.trim() == ".scratch/ux-audit-3/*.png")
        .expect(".gitignore 应有 ux-audit-3 的 glob 行");
    assert!(pos > 0, "glob 行之前应有注释行");
    let comment = lines[pos - 1];
    for keyword in ["UX_AUDIT3", "重新生成", "不入库"] {
        assert!(comment.contains(keyword), "注释缺「{keyword}」：{comment}");
    }
    assert!(
        gitignore.contains(".scratch/ux-audit/*.png")
            && gitignore.contains(".scratch/ux-audit-2/*.png"),
        "前两轮的截图排除先例应仍在"
    );

    // 工作区没有待提交的本轮二进制噪声（status 只对 PNG 断言——全量清洁属台账证据）。
    let (ok, status) = git_raw(&root, &["status", "--porcelain"]);
    assert!(ok, "git status 应可执行");
    let png_noise: Vec<&str> = status
        .lines()
        .filter(|l| l.contains(".scratch/ux-audit-3/") && l.ends_with(".png"))
        .collect();
    assert!(
        png_noise.is_empty(),
        "status 出现待提交的审计 PNG：{png_noise:?}"
    );
}

// ─────────────────────────── 场景 14（AC-5/AC-8） ───────────────────────────

/// harness 造不出的档不冒充「实测」；凡标「实测」必有可引用读数。
#[test]
fn scene_14_unfalsifiable_states_never_claimed_as_measured() {
    let root = root();
    let unseedable = ["context_overflow", "重试耗尽", "并发安装", "真重启"];
    for (rel, txt) in bodies_texts(&root) {
        let mentions_unseedable = unseedable.iter().any(|k| txt.contains(k));
        if mentions_unseedable {
            let level = field(&txt, "证据等级").unwrap_or_default();
            assert!(
                !level.contains("实测"),
                "{rel} 点名 harness 造不出的状态却标「实测」（R4/R5 虚报面）：{level}"
            );
        }
        let level = field(&txt, "证据等级").unwrap_or_default();
        if level.contains("实测") {
            assert!(
                txt.contains("[r3]") || !cited_pngs(&txt).is_empty(),
                "{rel} 标「实测」却无 [r3] 读数或截图名"
            );
        }
    }
}

// ─────────────────────────── 场景 15（AC-1/AC-2） ───────────────────────────

/// 任一中断点上都有一份自足清单：文件自足、状态列回填、无悬空引用、提交粒度无空目录态。
#[test]
fn scene_15_interruptible_selfcontained_checklist() {
    let root = root();
    let rels = body_rels(&root);
    assert_eq!(rels.len(), 13, "issues/ 下应有 13 份正文");
    assert!(root.join(INDEX_REL).exists(), "INDEX 在位");

    for (rel, txt) in bodies_texts(&root) {
        let nn = &rel.rsplit('/').next().unwrap()[..2];
        let title = txt.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        assert!(
            title.starts_with("# ") && title.contains(&format!("{nn}:")),
            "{rel} 缺自带标题「# {nn}: …」——单条自足、不依赖别处才看得懂"
        );
        // 四栏（场景 2 的自足子集）在本条内闭合。
        for label in ["出处", "严重度", "证据等级"] {
            assert!(field(&txt, label).is_some(), "{rel} 缺「{label}」——不自足");
        }
        assert!(rel_field(&txt).is_some(), "{rel} 缺「与前轮关联」——不自足");
        for dangling in ["见上面", "详见上面", "第 3 步的输出"] {
            assert!(
                !txt.contains(dangling),
                "{rel} 的结论指向别处的临时输出「{dangling}」——本地无对应内容"
            );
        }
    }

    // INDEX 状态列已随回填迁移（无「待核」残留）。
    let rows = index_rows(&read(&root, INDEX_REL));
    assert_eq!(rows.len(), 13, "INDEX 13 行");
    for row in &rows {
        let status = row
            .cells
            .get(5)
            .map(String::as_str)
            .unwrap_or("")
            .trim()
            .trim_matches('*');
        assert!(
            !status.is_empty() && status != "待核" && status != "待填",
            "候选 {} 的状态列未回填：{status}",
            row.nn
        );
    }

    // 提交粒度：凡触及 issues/ 的提交，该目录都非空（不存在只活在别处的中间态）。
    if !history_available(&root) {
        eprintln!("场景 15：历史不足——提交粒度的空目录断言跳过（台账见证据）");
        return;
    }
    let commits = git(&root, &["log", "--format=%H", "--", ISSUES_DIR])
        .expect("log issues")
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert!(!commits.is_empty(), "issues/ 应有提交历史");
    for commit in commits {
        let tree = git(
            &root,
            &["ls-tree", "--name-only", &commit, "--", ISSUES_DIR],
        )
        .expect("ls-tree 提交");
        assert!(
            !tree.trim().is_empty(),
            "提交 {} 的 issues/ 目录为空——该时点无可用清单",
            &commit[..7.min(commit.len())]
        );
    }
}

/// 一次性**落地形状**验收（`#[ignore]`，不进任何自动门）。
///
/// 收集本文件里按 `{merge-base}...HEAD`（**审计那一轮的分支 diff**）取数的两条读数：
/// 场景 5「决策 215/216/217 本轮一字未动」、场景 9「前两轮 spec 本轮零 diff」。这两条在
/// 审计合入 main 之后已经没有能重现它们的检出——留在常驻套件里就会在**每一个**任务分支上
/// 假红（2026-10-07 dogfood 实证同类：任务 01M4A35GGJ53YDJRZ0R3GZTM06 的 develop 闸门
/// 连红 4 轮）。故按「闸门套件必须分支无关」的不变量（`docs/testing.md` §8）摘出。
///
/// 重跑：`cargo test -p e2e --test integration artifact_shape_readings_once -- --ignored`
/// ——只在「分支 diff 恰为审计那一轮」的检出上会通过；别的分支上失败是**预期**。
#[test]
#[ignore = "一次性落地形状验收：锚在审计那一轮的分支 diff 上，不是分支无关的回归判据"]
fn artifact_shape_readings_once() {
    let root = root();
    let mb = merge_base(&root).expect("merge-base HEAD origin/main 应可得（本用例需完整历史）");
    let range = format!("{mb}...HEAD");

    // 场景 5：决策 215/216/217 本轮一字未动。
    let diff = git(
        &root,
        &["diff", "--stat", &range, "--", "docs/decisions.md"],
    )
    .expect("diff decisions");
    assert!(
        diff.trim().is_empty(),
        "本轮改了 docs/decisions.md——wontfix 裁决不得就地翻案：\n{diff}"
    );

    // 场景 9：前两轮 spec 本轮零 diff。
    let diff = git(
        &root,
        &[
            "diff",
            "--stat",
            &range,
            "--",
            "frontend/e2e/ux-audit.spec.ts",
            "frontend/e2e/ux-audit-2.spec.ts",
        ],
    )
    .expect("diff 既有 spec");
    assert!(
        diff.trim().is_empty(),
        "本轮改了前两轮 spec（抢救性修复已由决策 377① 于此前直推）：\n{diff}"
    );
}
