//! 集成测试：落地 ux-audit-3 的 13 条 UI 票 —— `test-scenarios.md` 场景 1–17 的产出物验收。
//!
//! 被测对象是 develop 阶段落盘的**前端产出物**（源文件 / 组件单测 / e2e 规格 / 实施记录）
//! 与 **git 可检的改动边界**；本文件不启动浏览器与 vitest——场景判据里「用例通过」的
//! 运行态证据（`cargo test`、`npm test`、`make check-lint`、svelte-check、build、
//! playwright 窄跑与复跑 spec）由 test 阶段用命令台账记入 `test-report.md`，
//! 静态断言负责把「产物存在、位置精确、边界不越」钉死（口径同决策 143 的可测性接缝）。
//!
//! `mask_comments` / `find_literal_stars` 是 `frontend/src/lib/copy-discipline.test.ts`
//! 里同名函数的 Rust 镜像，判据逐条对齐（三层剥注释：块 → 行（前一字符是 `:` 的不算，
//! 否则 `https://` 会吃掉行尾真命中）→ HTML；`[^*\n]+` 至少要一个非星非换行字符，
//! 故掩码 `***` 天然不命中）——这是场景 8「hits === []」的独立第二实现，两边一起烂时
//! 至少 Rust 这半边会红。
//!
//! **2026-10-10 处置（edf8b95 的后续）**：`.scratch/` 整体移出版本库后，读
//! `IMPLEMENTATION.md` 的场景 13–17 照 `landing_shape_readings_once` 的先例摘
//! `#[ignore]`（本地 `--ignored` 复跑）；产品侧行号/内容双钉的场景 9–11 照旧全跑，
//! 行号随 a0ece2a 在 TaskDetail 上方的 +1 行随迁。
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// ---------- 基础设施 ----------

fn root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // tests/e2e
    p.pop(); // tests
    p // 仓根
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("读 {rel} 失败: {e}"))
}

fn line_n(root: &Path, rel: &str, n: usize) -> String {
    read(root, rel)
        .lines()
        .nth(n - 1)
        .map(|s| s.to_string())
        .unwrap_or_else(|| panic!("{rel} 没有第 {n} 行——产出物行号漂移，按场景文档校对"))
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn git_out(root: &Path, args: &[&str]) -> String {
    git(root, args).unwrap_or_else(|| panic!("git {args:?} 返回非零"))
}

/// 变更面：`git status --porcelain`（含未跟踪）∪ `git diff --name-only HEAD` ∪ 分支相对基准的 diff。
///
/// **只服务 `#[ignore]` 的 [`landing_shape_readings_once`]**：它读的是「审计落地**那时**的分支
/// diff」这个**瞬时**状态——在别的分支上读到的就是**那个分支**的 diff。故它不属闸门套件
/// （闸门套件必须分支无关，见 `docs/testing.md` §8）。**常驻场景一律不得调用它。**
///
/// 「分支相对基准」用**三点** `origin/main...HEAD`（= `merge-base(origin/main, HEAD)..HEAD`）；
/// 两点 `origin/main..HEAD` 会把 main 侧邻接任务的改动一起算进来。
fn changed_paths(root: &Path) -> Vec<String> {
    let mut set = BTreeSet::new();
    if let Some(st) = git(root, &["status", "--porcelain"]) {
        for l in st.lines() {
            if l.len() < 4 {
                continue;
            }
            let p = l[3..].trim();
            let p = match p.split_once(" -> ") {
                Some((_, b)) => b,
                None => p,
            };
            set.insert(p.to_string());
        }
    }
    if let Some(d) = git(root, &["diff", "--name-only", "HEAD"]) {
        for l in d.lines() {
            let l = l.trim();
            if !l.is_empty() {
                set.insert(l.to_string());
            }
        }
    }
    // 提交落进任务分支后工作区变干净——改动面并上**分支相对基准**的 diff（场景 17 ① 的提交后时序）
    // 三点（merge-base..HEAD）：两点会把 main 侧邻接任务的改动也算成「本分支改动」。
    if let Some(d) = git(root, &["diff", "--name-only", "origin/main...HEAD"]) {
        for l in d.lines() {
            let l = l.trim();
            if !l.is_empty() {
                set.insert(l.to_string());
            }
        }
    }
    set.into_iter().collect()
}

/// `(新增行, 删除行)`；无 diff 时 (0, 0)。
///
/// **只服务 `#[ignore]` 的 [`landing_shape_readings_once`]**，理由同 [`changed_paths`]——
/// 按分支取数，不是分支无关判据。**常驻场景一律不得调用它。**
fn numstat(root: &Path, rel: &str) -> (usize, usize) {
    let mut out = git_out(root, &["diff", "--numstat", "HEAD", "--", rel]);
    if out.trim().is_empty() {
        // 变更落进任务分支后 `diff HEAD` 变空——回退到分支相对基准的读数（场景 17 ④ 的提交后时序）
        // 三点：两点会把 main 侧邻接任务的同路径改动也算进来，读数虚高。
        out = git_out(
            root,
            &["diff", "--numstat", "origin/main...HEAD", "--", rel],
        );
    }
    let first = out.lines().next().unwrap_or("0\t0\tpath");
    let mut it = first.split('\t');
    let add = it.next().unwrap_or("0").parse().unwrap_or(0);
    let del = it.next().unwrap_or("0").parse().unwrap_or(0);
    (add, del)
}

/// 断言 `needles` 按顺序全部出现在 `hay` 里（段间次序不可调换）。
fn assert_chain(what: &str, hay: &str, needles: &[&str]) {
    let mut pos = 0usize;
    for n in needles {
        let at = hay[pos..]
            .find(n)
            .unwrap_or_else(|| panic!("{what}: 缺段或次序不对 → {n}"));
        pos += at + n.len();
    }
}

/// 单测文件里从某用例标题到下一个 `it(` 之前的那一段（把断言圈在本用例体内）。
fn it_body<'a>(file: &'a str, marker: &str) -> &'a str {
    let at = file
        .find(marker)
        .unwrap_or_else(|| panic!("缺用例标题 → {marker}"));
    let rest = &file[at..];
    let end = rest.find("\n  it(").unwrap_or(rest.len());
    &rest[..end]
}

/// 审计冻结面：第一轮已验收的 README / 13 张票 / 复跑 spec，本票一个字节不许动。
const FROZEN: &[&str] = &[
    ".scratch/ux-audit-3/README.md",
    ".scratch/ux-audit-3/issues",
    "frontend/e2e/ux-audit-3.spec.ts",
];

fn assert_frozen_untouched(root: &Path) {
    let mut args: Vec<&str> = vec!["status", "--porcelain", "--"];
    args.extend_from_slice(FROZEN);
    let st = git_out(root, &args);
    assert!(st.trim().is_empty(), "审计冻结文件出现在改动面: {st}");
    let mut args: Vec<&str> = vec!["diff", "--name-only", "HEAD", "--"];
    args.extend_from_slice(FROZEN);
    let d = git_out(root, &args);
    assert!(d.trim().is_empty(), "审计冻结文件被改: {d}");
}

// ---------- 字面强调星号扫描（copy-discipline.test.ts 的 Rust 镜像） ----------

/// 三层剥注释：块 → 行（`//` 前是 `:` 的不算，`https://…` 不吃行尾）→ HTML（仅 .svelte）。
/// 非换行字符一律换成空格，**长度与行号不变**，故命中还能对回原文。
fn mask_comments(text: &str, html: bool) -> Vec<char> {
    let mut c: Vec<char> = text.chars().collect();
    let n = c.len();
    // ① 块注释（含 svelte 的 {/* … *}）
    let mut i = 0;
    while i + 1 < n {
        if c[i] == '/' && c[i + 1] == '*' {
            let mut j = i + 2;
            while j + 1 < n && !(c[j] == '*' && c[j + 1] == '/') {
                j += 1;
            }
            if j + 1 >= n {
                // 与 JS 正则一致：没有闭合就不算匹配，跳过这段头继续找下一个 /*
                i += 2;
                continue;
            }
            for ch in c.iter_mut().take(j + 2).skip(i) {
                if *ch != '\n' {
                    *ch = ' ';
                }
            }
            i = j + 2;
        } else {
            i += 1;
        }
    }
    // ② 行注释
    let mut i = 0;
    while i + 1 < n {
        if c[i] == '/' && c[i + 1] == '/' && (i == 0 || c[i - 1] != ':') {
            let mut j = i;
            while j < n && c[j] != '\n' {
                j += 1;
            }
            for ch in c.iter_mut().take(j).skip(i) {
                *ch = ' ';
            }
            i = j;
        } else {
            i += 1;
        }
    }
    // ③ HTML 注释（仅 .svelte）
    if html {
        let mut i = 0;
        while i + 3 < n {
            if c[i] == '<' && c[i + 1] == '!' && c[i + 2] == '-' && c[i + 3] == '-' {
                let mut j = i + 4;
                while j + 2 < n && !(c[j] == '-' && c[j + 1] == '-' && c[j + 2] == '>') {
                    j += 1;
                }
                if j + 2 >= n {
                    i += 4;
                    continue;
                }
                for ch in c.iter_mut().take(j + 3).skip(i) {
                    if *ch != '\n' {
                        *ch = ' ';
                    }
                }
                i = j + 3;
            } else {
                i += 1;
            }
        }
    }
    c
}

/// `\*\*[^*\n]+\*\*`：至少一个非星非换行字符夹在中间才算。
/// 返回 (1 起行号, 原文该行摘录 ≤110 字符)。
fn find_literal_stars(text: &str, html: bool) -> Vec<(usize, String)> {
    let masked = mask_comments(text, html);
    let raw: Vec<&str> = text.split('\n').collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 1 < masked.len() {
        if masked[i] == '*' && masked[i + 1] == '*' {
            let mut j = i + 2;
            while j < masked.len() && masked[j] != '*' && masked[j] != '\n' {
                j += 1;
            }
            if j - (i + 2) >= 1 && j + 1 < masked.len() && masked[j] == '*' && masked[j + 1] == '*'
            {
                let line = masked[..i].iter().filter(|&&ch| ch == '\n').count() + 1;
                let snippet: String = raw
                    .get(line - 1)
                    .map(|s| s.trim().chars().take(110).collect())
                    .unwrap_or_default();
                out.push((line, snippet));
                i = j + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// 扫描面（同 vitest 门）：`frontend/src` 下 `.svelte` / `.ts`，测试与 bench 文件除外。
fn collect_scan_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if name == "node_modules" {
                continue;
            }
            collect_scan_files(&p, out);
        } else if name.ends_with(".svelte")
            || (name.ends_with(".ts")
                && !name.ends_with(".test.ts")
                && !name.ends_with(".spec.ts")
                && !name.ends_with(".bench.ts"))
        {
            out.push(p);
        }
    }
}

fn scan_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_scan_files(&root.join("frontend/src"), &mut files);
    files.sort();
    files
}

fn literal_star_hits(root: &Path) -> Vec<String> {
    let mut hits = Vec::new();
    for f in scan_files(root) {
        let rel = f.strip_prefix(root).unwrap_or(&f).display().to_string();
        let text = match fs::read_to_string(&f) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let html = f.extension().map(|e| e == "svelte").unwrap_or(false);
        for (ln, sn) in find_literal_stars(&text, html) {
            hits.push(format!("{rel}:{ln}: {sn}"));
        }
    }
    hits
}

// ---------- 场景 1–17 ----------

/// 场景 1（AC-1 / 票 05）：Escape 关闭焦点所在的 toast——组件守卫链 + 组件级单测两半。
#[test]
fn scene_01_escape_closes_the_focused_toast() {
    let root = root();
    // ① 源码：window 级 keydown 订阅 + 六段守卫链，次序钉死（先看 key → 有无 toast →
    //    焦点归属 → 稳定定位器取 id → 数字校验 → preventDefault → 精确关闭）
    let src = read(&root, "frontend/src/components/layout/ToastStack.svelte");
    assert!(
        src.contains("<svelte:window onkeydown={handleKey} />"),
        "场景 1：ToastStack 必须在 window 上订阅 keydown"
    );
    assert!(
        src.contains("bind:this={stack}"),
        "场景 1：归属守卫需要 .toasts 容器引用"
    );
    assert!(
        src.contains("data-toast-id={toast.id}"),
        "场景 1：toast 根元素要挂稳定定位器"
    );
    assert_chain(
        "场景 1 票 05 Escape 守卫链",
        &src,
        &[
            "if (e.key !== 'Escape' || notifications.toasts.length === 0) return;",
            "if (!stack || !active || !stack.contains(active)) return;",
            "const raw = active.closest('[data-toast-id]')?.getAttribute('data-toast-id') ?? null;",
            "if (raw === null || !Number.isInteger(Number(raw))) return;",
            "e.preventDefault();",
            "notifications.dismiss(Number(raw));",
        ],
    );
    // ② 组件级单测：同一条 id 同时断 DOM 与 store（场景判据的两半）
    let unit = read(&root, "frontend/src/components/layout/ToastStack.test.ts");
    let body = it_body(&unit, "焦点在关闭钮上按 Escape");
    assert!(
        body.contains("expect(document.activeElement).toBe(close)"),
        "场景 1：用例必须先把焦点放到关闭钮"
    );
    assert!(
        body.contains("await fireEvent.keyDown(window, { key: 'Escape' });"),
        "场景 1：用例必须真发 Escape"
    );
    assert!(
        body.contains("expect(toastEl(id)).toBeNull()"),
        "场景 1：DOM 侧判据缺失"
    );
    assert!(
        body.contains("expect(notifications.toasts.some((t) => t.id === id)).toBe(false)"),
        "场景 1：store 侧判据缺失"
    );
}

/// 场景 2（AC-1 / 票 05）：焦点在 toast 外按 Escape——归属守卫，一个字节都不动。
#[test]
fn scene_02_escape_outside_the_stack_touches_nothing() {
    let root = root();
    let src = read(&root, "frontend/src/components/layout/ToastStack.svelte");
    assert!(
        src.contains("if (!stack || !active || !stack.contains(active)) return;"),
        "场景 2：焦点归属守卫缺失"
    );
    let unit = read(&root, "frontend/src/components/layout/ToastStack.test.ts");
    let body = it_body(&unit, "焦点在 toast 外（document.body）按 Escape");
    assert!(
        body.contains("expect(document.activeElement).toBe(document.body);"),
        "场景 2：用例必须把焦点放到 body"
    );
    assert!(
        body.contains("expect(toastEl(id), '焦点不在 toast 里时 Escape 不该关它').not.toBeNull();"),
        "场景 2：DOM 不动的判据缺失"
    );
    assert!(
        body.contains("expect(notifications.toasts.some((t) => t.id === id)).toBe(true);"),
        "场景 2：store 不动的判据缺失"
    );
}

/// 场景 3（AC-1 / 票 05）：两条 toast 只关焦点所在的那一条（归属靠稳定定位器）。
#[test]
fn scene_03_two_toasts_only_close_the_focused_one() {
    let root = root();
    let src = read(&root, "frontend/src/components/layout/ToastStack.svelte");
    assert!(
        src.contains("active.closest('[data-toast-id]')"),
        "场景 3：归属必须走 data-toast-id，不吃「第 N 条」的歧义"
    );
    let unit = read(&root, "frontend/src/components/layout/ToastStack.test.ts");
    let body = it_body(&unit, "两条 toast 时只关焦点所在的那一条");
    assert!(
        body.contains("closeBtn(second).focus();"),
        "场景 3：焦点要精确放到第二条"
    );
    assert!(
        body.contains("expect(toastEl(first), '不该连坐关掉另一条').not.toBeNull();"),
        "场景 3：另一条 DOM 保留的判据缺失"
    );
    assert!(
        body.contains("expect(notifications.toasts.map((t) => t.id)).toEqual([first]);"),
        "场景 3：store 只剩第一条的判据缺失"
    );
}

/// 场景 4（AC-1 / 票 05）：没有 toast 时按 Escape 不抛错（守卫第一句即 return）。
#[test]
fn scene_04_escape_with_no_toast_is_a_noop() {
    let root = root();
    let src = read(&root, "frontend/src/components/layout/ToastStack.svelte");
    assert_chain(
        "场景 4 守卫先于取焦点",
        &src,
        &[
            "if (e.key !== 'Escape' || notifications.toasts.length === 0) return;",
            "const active = document.activeElement;",
        ],
    );
    let unit = read(&root, "frontend/src/components/layout/ToastStack.test.ts");
    let body = it_body(&unit, "没有 toast 时按 Escape 不抛错");
    assert_eq!(
        body.matches("toHaveLength(0)").count(),
        2,
        "场景 4：发键前后各一次空数组断言（handler 抛错会在这里冒出来）"
    );
    assert!(
        body.contains("fireEvent.keyDown(window, { key: 'Escape' })"),
        "场景 4：用例必须真发 Escape"
    );
}

/// 场景 5（AC-2 / 票 05）：点击 / Enter 路径一条不丢——新键路径是加法，不减老路径。
#[test]
fn scene_05_close_button_paths_unchanged() {
    let root = root();
    let src = read(&root, "frontend/src/components/layout/ToastStack.svelte");
    assert!(
        src.contains("type=\"button\""),
        "场景 5：关闭钮仍是原生 button"
    );
    assert!(
        src.contains("onclick={() => notifications.dismiss(toast.id)}"),
        "场景 5：点击关闭路径被改动"
    );
    for h in [
        "onmouseenter={() => notifications.pause(toast.id)}",
        "onmouseleave={() => notifications.resume(toast.id)}",
        "onfocusin={() => notifications.pause(toast.id)}",
        "onfocusout={() => notifications.resume(toast.id)}",
    ] {
        assert!(
            src.contains(h),
            "场景 5：hover/focus 暂停恢复丢了一条 → {h}"
        );
    }
    let unit = read(&root, "frontend/src/components/layout/ToastStack.test.ts");
    assert_eq!(
        unit.matches("\n  it(").count(),
        6,
        "场景 5：组件单测共 6 例（5 条 Escape 分派 + 点击回归 + Enter 不误伤，逐条在册）"
    );
    let click = it_body(&unit, "关闭钮的点击路径回归照旧");
    assert!(
        click.contains("fireEvent.click"),
        "场景 5：点击回归用例缺失"
    );
    let enter = it_body(&unit, "Escape 不误伤 Enter 等其它键");
    assert!(
        enter.contains("key: 'Enter'"),
        "场景 5：非 Escape 键用例缺失"
    );
}

/// 场景 6（AC-2 / 票 05）：真页面 e2e——聚焦关闭钮 → Escape → 该条从 DOM 消失。
#[test]
fn scene_06_real_page_e2e_escape_assertion_present() {
    let root = root();
    let spec = read(&root, "frontend/e2e/ux2-resilience.spec.ts");
    let at = spec
        .find("const toastId = await toast.getAttribute('data-toast-id');")
        .expect("场景 6：ux2-resilience 缺真页面 Escape 断言块");
    let tail = &spec[at..];
    assert!(
        spec.contains("票 05（ux-audit-3）"),
        "场景 6：断言块必须写明归票"
    );
    assert_chain(
        "场景 6 真页面 Escape 断言",
        tail,
        &[
            "expect(toastId, 'toast 应当带 data-toast-id（稳定定位器）').not.toBeNull();",
            "await toast.locator('button.close').focus();",
            "await page.keyboard.press('Escape');",
            "`.toast[data-toast-id=\"${toastId}\"]`",
            "toHaveCount(0);",
        ],
    );
}

/// 场景 7（AC-3 / 票 08）：四字面 `**…**` 换成 `<b>`，位置逐处钉行号。
#[test]
fn scene_07_four_literal_stars_become_b_elements() {
    let root = root();
    let notify = "frontend/src/routes/SettingsNotify.svelte";
    let tools = "frontend/src/routes/SettingsTools.svelte";
    assert!(
        line_n(&root, notify, 577).contains("保存的是<b>整体覆盖</b>"),
        "场景 7：SettingsNotify:577 应为 <b> 形态"
    );
    assert!(
        line_n(&root, notify, 589).contains("<b>同一条通知每台订阅设备各收一份</b>"),
        "场景 7：SettingsNotify:589 应为 <b> 形态"
    );
    assert!(
        line_n(&root, notify, 676).contains("endpoint 的<b>摘要</b>"),
        "场景 7：SettingsNotify:676 应为 <b> 形态"
    );
    assert!(
        // 行号随决策 398 的白名单模式段（script 加了 ~40 行）下移；行号钉住的是
        // 「这一页的字面加粗都在 <b> 形态」的当下事实，文件再加段要跟着挪。
        line_n(&root, tools, 202).contains("<b>不改写</b>"),
        "场景 7：SettingsTools:202 应为 <b> 形态"
    );
    // 字面星号形态彻底退场（注释里的 ** 照旧，故只查这四处旧文案本身）
    let n = read(&root, notify);
    for old in [
        "保存的是**整体覆盖**",
        "**同一条通知每台订阅设备各收一份**",
        "endpoint 的**摘要**",
    ] {
        assert!(!n.contains(old), "场景 7：旧字面形态残留 → {old}");
    }
    assert!(
        !read(&root, tools).contains("**不改写**"),
        "场景 7：旧字面形态残留 → **不改写**"
    );
    // 「只修这四处：删除量恰好 3 + 1」是**落地那时**的分支 diff 读数，已摘到
    // `#[ignore]` 的 `landing_shape_readings_once`（按分支取数不是闸门判据，见该用例）。
    // 实测读数（SettingsNotify (3,3) / SettingsTools (1,1)，落地提交 d7bfcc9）留档于
    // `.scratch/ux-audit-3/IMPLEMENTATION.md`；本场景按**文件内容**的判据照旧全跑。
}

/// 场景 8（AC-3 / 票 08）：copy-discipline 机器门——四用例 + 全站扫描 hits === []（Rust 独立复算）。
#[test]
fn scene_08_copy_discipline_machine_gate() {
    let root = root();
    // ① 正反例自检：本文件的 Rust 镜像逐条复刻 vitest 判据
    assert_eq!(
        find_literal_stars("<p>第一行</p>\n<p>保存的是**整体覆盖**</p>", true),
        vec![(2, "<p>保存的是**整体覆盖**</p>".to_string())],
        "场景 8：正例应 1 处、行号 2"
    );
    assert_eq!(
        find_literal_stars("note('闸门**不改写**')", false).len(),
        1,
        "场景 8：.ts 字符串字面量同样算"
    );
    assert_eq!(
        find_literal_stars("// **照旧**", false).len(),
        0,
        "场景 8：行注释不算"
    );
    assert_eq!(
        find_literal_stars("/* **照旧**\n   续行 */", false).len(),
        0,
        "场景 8：块注释不算（跨行）"
    );
    assert_eq!(
        find_literal_stars("<!-- **照旧** -->", true).len(),
        0,
        "场景 8：HTML 注释不算"
    );
    assert_eq!(
        find_literal_stars("{/* **照旧** */}\n<p>正文</p>", true).len(),
        0,
        "场景 8：svelte 块注释不算"
    );
    assert_eq!(
        find_literal_stars("const NOTIFY_SECRET_MASK = '***';", false).len(),
        0,
        "场景 8：掩码 *** 不计（审计 ⑤.2 的订正）"
    );
    assert_eq!(
        find_literal_stars("<li>***（掩码）</li>", true).len(),
        0,
        "场景 8：掩码 *** 不计"
    );
    assert_eq!(
        find_literal_stars("<p>读回 *** </p>", true).len(),
        0,
        "场景 8：掩码 *** 不计"
    );
    // ② 全站扫描（场景判据 hits === []）——独立第二实现复算
    let files = scan_files(&root);
    assert!(
        files.len() > 50,
        "场景 8：扫描面应 >50 文件，实得 {}",
        files.len()
    );
    assert!(
        files.iter().all(|f| {
            let n = f.file_name().unwrap().to_string_lossy().into_owned();
            !(n.ends_with(".test.ts") || n.ends_with(".spec.ts") || n.ends_with(".bench.ts"))
        }),
        "场景 8：测试与 bench 文件不在扫描面"
    );
    let hits = literal_star_hits(&root);
    assert!(
        hits.is_empty(),
        "场景 8：全站扫描应 0 处命中，实得 {} 处：{:?}",
        hits.len(),
        hits
    );
    // ③ vitest 门自身在位：导出面 + 四用例标题
    let copy = read(&root, "frontend/src/lib/copy-discipline.test.ts");
    assert!(
        copy.contains("const LITERAL_BOLD = /\\*\\*[^*\\n]+\\*\\*/g;"),
        "场景 8：vitest 门的正则缺失"
    );
    assert!(
        copy.contains("export function findLiteralStars"),
        "场景 8：findLiteralStars 导出缺失"
    );
    for title in [
        "全站没有一处 `**…**` 落在面向用户的位置（hits === []）",
        "注释照旧不算 / 测试文件不在扫描面（199 同一边界）",
        "掩码 `***` 是有意设计，不计（回归审计 ⑤.2 的订正）",
        "模板正文里的算（正例：`<p>保存的是**整体覆盖**</p>` → 1 处，带行号与 snippet）",
    ] {
        assert!(copy.contains(title), "场景 8：vitest 用例缺 → {title}");
    }
}

/// 场景 9（AC-4 / 票 13）：820–1099 折行档——右栏 280、主栏 ≥480（源码 + e2e 两半）。
#[test]
fn scene_09_detail_fold_band_820_1099() {
    let root = root();
    let td = read(&root, "frontend/src/routes/TaskDetail.svelte");
    assert_chain(
        "场景 9 决策 215 折行档",
        &td,
        &[
            "/* 决策 215 的 820–1099 档",
            "@media (min-width: 820px) and (max-width: 1099px) {",
            ".detail.split {",
            "grid-template-columns: minmax(480px, 1fr) 280px;",
        ],
    );
    assert_eq!(
        line_n(&root, "frontend/src/routes/TaskDetail.svelte", 681).trim(),
        "@media (min-width: 820px) and (max-width: 1099px) {",
        "场景 9：媒体查询落在 681 行（决策 393 之前 677；决策 215 的块体随迁 672–684；评审台账 a0ece2a 于上方 +1 行 → 681，块体 673–685）"
    );
    let geo = read(&root, "frontend/e2e/ux2-geometry.spec.ts");
    assert!(
        geo.contains("票 13（ux-audit-3）：详情页 820–1099 折行档"),
        "场景 9：geometry 新用例缺归票注释"
    );
    assert_chain(
        "场景 9 geometry 用例",
        &geo,
        &[
            "for (const w of [1099, 1024, 900, 820]) {",
            "toBe('280px')",
            "toBeGreaterThanOrEqual(480)",
        ],
    );
}

/// 场景 10（AC-4 / 票 13）：边界格 1100 / 819 守两侧 320px——min-width 钉死 820。
#[test]
fn scene_10_band_boundaries_1100_and_819() {
    let root = root();
    let rel = "frontend/src/routes/TaskDetail.svelte";
    let td = read(&root, rel);
    assert_eq!(
        td.matches("@media (min-width: 820px) and (max-width: 1099px)")
            .count(),
        1,
        "场景 10：折行档媒体查询恰一处"
    );
    assert!(
        !td.contains("(min-width: 819px)"),
        "场景 10：下界不许是 819——819 仍是票 01 的 wontfix 面"
    );
    assert!(
        !td.contains("(max-width: 1100px)"),
        "场景 10：上界不许吃进 1100（桌面档）"
    );
    // <479 档的 display:block 双保险仍在原位（票 01 面一格不动的源码侧牙齿）
    assert_eq!(
        line_n(&root, rel, 895).trim(),
        ".detail.split {",
        "场景 10：<479 档 .detail.split 规则位移（a0ece2a 于上方 +1 行，894 → 895）"
    );
    assert_eq!(
        line_n(&root, rel, 896).trim(),
        "display: block;",
        "场景 10：<479 档 display:block 被动"
    );
    let geo = read(&root, "frontend/e2e/ux2-geometry.spec.ts");
    assert_chain(
        "场景 10 边界档用例",
        &geo,
        &["for (const w of [1100, 819]) {", "toBe('320px')"],
    );
    assert!(
        geo.contains("票 01 面一格不动"),
        "场景 10：边界档用例要把意图写进断言消息"
    );
}

/// 场景 11（AC-4 / 票 13）：主带规则不变——TaskDetail 纯 13 行新增、hero 与桌面档不动。
#[test]
fn scene_11_detail_other_rules_untouched() {
    let root = root();
    let rel = "frontend/src/routes/TaskDetail.svelte";
    // 「本票只许增 13 行、新增行含媒体查询/折行档列串、不引 overflow」是**落地那时**的分支
    // diff 读数，已摘到 `#[ignore]` 的 `landing_shape_readings_once`（按分支取数不是闸门
    // 判据）。实测读数（(13,0)，落地提交 116745b）留档于
    // `.scratch/ux-audit-3/IMPLEMENTATION.md`；下面按**文件内容**的行号/内容双钉照旧全跑。
    // 既有的 `overflow-x: auto`（决策 393 前在 961 行的预存在规则）必须落在本票改动块
    // （决策 393 随迁 672–684；a0ece2a +1 行 → 现 673–685）之外
    let td = read(&root, rel);
    let overflow_lines: Vec<usize> = td
        .lines()
        .enumerate()
        .filter(|(_, l)| l.contains("overflow-x"))
        .map(|(i, _)| i + 1)
        .collect();
    assert!(
        overflow_lines.iter().all(|n| !(673..=685).contains(n)),
        "场景 11：overflow-x 渗进本票改动块（行号随决策 393 / a0ece2a 随迁）→ 行 {overflow_lines:?}"
    );
    // 桌面 / 窄档既有规则逐条在原位（行号 + 内容双钉；a0ece2a 于上方 +1 行）
    assert_eq!(
        line_n(&root, rel, 667).trim(),
        "max-width: 1240px;",
        "场景 11：桌面档 max-width 被动"
    );
    assert_eq!(
        line_n(&root, rel, 669).trim(),
        "grid-template-columns: minmax(0, 1fr) 320px;",
        "场景 11：桌面档列串被动"
    );
    assert_eq!(
        line_n(&root, rel, 670).trim(),
        "gap: 18px;",
        "场景 11：gap 被动"
    );
    assert_eq!(
        line_n(&root, rel, 671).trim(),
        "align-items: start;",
        "场景 11：align-items 被动"
    );
    // 「hero 轨道 PipelineRail 不在改动面」同样按分支取数，已摘到 `landing_shape_readings_once`。
}

/// 场景 12（AC-4 / 票 13）：geometry 新用例不测横向溢出（裁量写进注释，断言只管列串）。
#[test]
fn scene_12_geometry_test_has_no_overflow_assertion() {
    let root = root();
    let geo = read(&root, "frontend/e2e/ux2-geometry.spec.ts");
    let at = geo
        .find("票 13（ux-audit-3）：详情页 820–1099 折行档")
        .expect("场景 12：缺票 13 归票注释块");
    let blk = &geo[at..];
    assert!(
        blk.contains("**不断言**页面横向溢出"),
        "场景 12：不测溢出的裁量必须写在用例注释里（820 恒溢 12px 属票 01 wontfix）"
    );
    for forbidden in ["scrollWidth", "clientWidth", "toBeLessThanOrEqual(0)"] {
        assert!(
            !blk.contains(forbidden),
            "场景 12：新用例不许断横向溢出 → 命中 {forbidden}"
        );
    }
    assert!(
        blk.contains("expectBundleHealthy(bundle);"),
        "场景 12：用例仍应做 bundle 健康检查"
    );
}

/// 场景 13（AC-5 / 复核票）：票 02/06/11/12 复核后关闭，探针值与 14 passed 复跑留档；冻结面不动。
#[test]
#[ignore = "锚在 .scratch/ux-audit-3/IMPLEMENTATION.md 上（edf8b95 起移出版本库，闸门克隆无此目录）：本地 --ignored 复跑；判据不是分支无关回归"]
fn scene_13_recheck_tickets_recorded_and_frozen() {
    let root = root();
    let impl_md = read(&root, ".scratch/ux-audit-3/IMPLEMENTATION.md");
    for sec in [
        "### 票 02 · 对讲台中间档折行（已修） —— **复核即关**",
        "### 票 06 · 第一轮 B 叠实现票是否真落（已修） —— **复核即关**",
        "### 票 11 · 决策 240 入口归一 wordmark 去链（已修） —— **验无回归后关**",
        "### 票 12 · 决策 243/300 窄档底栏与状态条（已修） —— **验无回归后关**",
    ] {
        assert!(impl_md.contains(sec), "场景 13：记录缺段 → {sec}");
    }
    assert!(
        impl_md.contains("**14 passed**"),
        "场景 13：复跑 spec 结果必须留档"
    );
    assert!(
        impl_md.contains("900 → 562px 280px"),
        "场景 13：票 02 逐格探针值缺失"
    );
    assert!(
        impl_md.contains("{\"tag\":\"SPAN\",\"isLink\":false"),
        "场景 13：票 11 wordmark 探针值缺失"
    );
    assert!(
        impl_md.contains("--sbar-h = calc(58px + 0px)"),
        "场景 13：票 12 状态条探针值缺失"
    );
    assert_frozen_untouched(&root);
}

/// 场景 14（AC-6 / 新开票）：票 07/09/10 新开无缺陷——记录即关，探针值入档。
#[test]
#[ignore = "锚在 .scratch/ux-audit-3/IMPLEMENTATION.md 上（edf8b95 起移出版本库，闸门克隆无此目录）：本地 --ignored 复跑；判据不是分支无关回归"]
fn scene_14_new_tickets_recorded_as_no_defect() {
    let root = root();
    let impl_md = read(&root, ".scratch/ux-audit-3/IMPLEMENTATION.md");
    for sec in [
        "### 票 07 · 命令执行设置页（新开无缺陷） —— **记录即关**",
        "### 票 09 · 值守轮设置页（新开无缺陷） —— **记录即关**",
        "### 票 10 · 看板顶部道具栏（新开无缺陷） —— **记录即关**",
    ] {
        assert!(impl_md.contains(sec), "场景 14：记录缺段 → {sec}");
    }
    assert!(
        impl_md.contains("`main=1`、恰一个 `h1`（`设置 · 命令执行`）"),
        "场景 14：票 07 探针值缺失"
    );
    assert!(
        impl_md.contains("`main=1`、恰一个 `h1`（`设置 · 值守轮`）"),
        "场景 14：票 09 探针值缺失"
    );
    assert!(
        impl_md.contains("`slotCount=7`"),
        "场景 14：票 10 探针值缺失"
    );
    assert!(
        impl_md.contains("`hasCount=null`"),
        "场景 14：票 10 第 3 槽探针值缺失"
    );
    // 「记录即关」= 无缺陷 ⇒ 记录里不含「动代码」字样（与票 05/08/13 的「关（动代码）」相对）
    for sec in ["### 票 07", "### 票 09", "### 票 10"] {
        let at = impl_md.find(sec).expect("场景 14：缺段");
        let rest = &impl_md[at + sec.len()..];
        let end = rest.find("\n### ").unwrap_or(rest.len());
        let sect = &rest[..end];
        assert!(
            !sect.contains("动代码"),
            "场景 14：新开无缺陷票 {sec} 不该带「动代码」"
        );
    }
}

/// 场景 15（AC-6 / wontfix）：票 01/03/04 维持 wontfix——记档 + 无回归 + 溢出列证据。
#[test]
#[ignore = "锚在 .scratch/ux-audit-3/IMPLEMENTATION.md 上（edf8b95 起移出版本库，闸门克隆无此目录）：本地 --ignored 复跑；判据不是分支无关回归"]
fn scene_15_wontfix_tickets_kept_and_code_untouched() {
    let root = root();
    let impl_md = read(&root, ".scratch/ux-audit-3/IMPLEMENTATION.md");
    for sec in [
        "### 票 01 · 详情页 480–819 中间档折行与 hero 溢出 —— **维持 wontfix（有意不做），无回归**",
        "### 票 03 · 破坏性动作无确认步（wontfix） —— **维持 wontfix，不重开**",
        "### 票 04 · 中流状态持久化余三件（wontfix） —— **维持 wontfix，不重开**",
    ] {
        assert!(impl_md.contains(sec), "场景 15：记录缺段 → {sec}");
    }
    assert!(
        impl_md.contains("溢出列 `820→12、768→64、600→232、520→312、480→352`"),
        "场景 15：票 01 溢出列「同值」证据缺失"
    );
    assert!(
        impl_md.contains("819 及以下仍读 `320px`"),
        "场景 15：票 01 双保险「819 仍 320px」证据缺失"
    );
    assert_frozen_untouched(&root);
}

/// 场景 16（AC-7 / 闸门）：五条本地闸门结果入档（本阶段另行实跑，命令台账见 test-report.md）。
#[test]
#[ignore = "锚在 .scratch/ux-audit-3/IMPLEMENTATION.md 上（edf8b95 起移出版本库，闸门克隆无此目录）：本地 --ignored 复跑；判据不是分支无关回归"]
fn scene_16_gate_summary_documented() {
    let root = root();
    let impl_md = read(&root, ".scratch/ux-audit-3/IMPLEMENTATION.md");
    for row in [
        "| `make check-lint`（fmt + clippy） | 通过（Rust 零改动） |",
        "| `cd frontend && npm test` | 84 文件 / **1171 例全通过**",
        "| `cd frontend && npm run check` | 0 errors / 0 warnings |",
        "| `cd frontend && npm run build` | 通过 |",
        "**11 passed / 0 failed**（两条新用例定点复跑亦 PASS）",
        "| 复跑 `ux-audit-3.spec.ts`（证据，非门） | **14 passed** |",
    ] {
        assert!(impl_md.contains(row), "场景 16：闸门台账缺行 → {row}");
    }
    assert!(
        impl_md.contains("UX_AUDIT3=1 AGENTPIPELINE_E2E_BIN="),
        "场景 16：复跑命令（含共享 target 出口）必须可照抄"
    );
}

/// 场景 17（AC-7 / 改动范围）：产品改动只落 frontend/src + frontend/e2e；证据文件冻结；四段 message 可反查。
#[test]
#[ignore = "锚在 .scratch/ux-audit-3/IMPLEMENTATION.md 上（edf8b95 起移出版本库，闸门克隆无此目录）：本地 --ignored 复跑；判据不是分支无关回归"]
fn scene_17_change_scope_and_evidence_freeze() {
    let root = root();
    let impl_md = read(&root, ".scratch/ux-audit-3/IMPLEMENTATION.md");
    // ① 改动面白名单 + ②「冻结文件不在改动面」：两条都按**落地那次的分支 diff** 取数，
    //    已摘到 `#[ignore]` 的 `landing_shape_readings_once`（按分支取数不是闸门判据——
    //    在任何任务分支上读到的都是**那个任务**的 diff，见 `docs/testing.md` §8）。
    // ②′ 冻结面仍留一道**分支无关**的实断言：工作区里不许有冻结文件的未提交改动。
    assert_frozen_untouched(&root);
    // ③ 证据文件在位：实施记录 + 两张截图——**两张都可能缺**。`.gitignore:30` 明写
    //    `.scratch/ux-audit-3/*.png` 不入库（`UX_AUDIT3=1` 跑 e2e/ux-audit-3.spec.ts 可重生成），
    //    故全新 checkout（CI 就是）里两张都不会在——把「截图没入库」当缺陷红是判据错位。
    //    缺哪张就要求 IMPLEMENTATION.md 里有环境差异记录（措辞沿用票面 08 既有那一处），
    //    于是「既没证据、也没说明」仍然拦得住。
    assert!(
        root.join(".scratch/ux-audit-3/IMPLEMENTATION.md").is_file(),
        "场景 17：IMPLEMENTATION.md 应在"
    );
    let env_difference_recorded =
        impl_md.contains("修前截图备份无源文件") && impl_md.contains("无源可备");
    for rel in [
        "r3-literal-asterisks.pre-fix.png",
        "r3-literal-asterisks.png",
    ] {
        let path = format!(".scratch/ux-audit-3/{rel}");
        if !root.join(&path).is_file() {
            assert!(
                env_difference_recorded,
                "场景 17：{path} 不存在，必须有环境差异记录（审计截图不入库，见 .gitignore:30）"
            );
        }
    }
    // ④ 四段提交 message 反查票面——同样按 `{merge-base}..HEAD` 取数，已摘到
    //    `landing_shape_readings_once`（落地那次的四段 message：票 05 / 08 / 13 + ux-audit-3）。
}

/// 一次性**落地形状**验收（`#[ignore]`，不进任何自动门）。
///
/// 这里收集的是「ux-audit-3 那次落地**当时**的分支 diff 形状」判据：改动面白名单、逐文件
/// 增删行数、新增行内容、hero 轨道是否被碰、四段 message 反查。它们的锚点是**落地那一次
/// 的分支 diff**，不是任何持久状态——审计合入 main 之后（且 main 上还夹着别的提交），
/// 已不存在任何提交区间能让它们成立。
///
/// 留在常驻套件里就会在**每一个**任务分支上假红，把 develop / merge 的 `cargo test` 闸门
/// 整体卡死：2026-10-07 dogfood（任务 01M4A35GGJ53YDJRZ0R3GZTM06）连红 4 轮实证。
/// 故按「闸门套件必须分支无关」的不变量（`docs/testing.md` §8）摘出为 `#[ignore]`。
///
/// 落地时的实测读数留档于 `.scratch/ux-audit-3/IMPLEMENTATION.md`（「落地形状读数」一节）：
/// SettingsNotify (3,3) / SettingsTools (1,1)（提交 `d7bfcc9`）、TaskDetail (13,0)（`116745b`）。
///
/// 重跑：`cargo test -p e2e --test integration landing_shape_readings_once -- --ignored`
/// ——只在「分支 diff 恰为那次审计落地」的检出上会通过（例如把审计那几段 cherry-pick 到
/// 一个干净基准上）；在任何别的分支上失败是**预期**，不是回归。
#[test]
#[ignore = "一次性落地形状验收：锚在审计落地那次的分支 diff 上，不是分支无关的回归判据"]
fn landing_shape_readings_once() {
    let root = root();
    let notify = "frontend/src/routes/SettingsNotify.svelte";
    let tools = "frontend/src/routes/SettingsTools.svelte";
    let td = "frontend/src/routes/TaskDetail.svelte";

    // 场景 7：只修这四处——删除量恰好 3 + 1
    assert_eq!(
        numstat(&root, notify),
        (3, 3),
        "场景 7：SettingsNotify 应恰改 3 行"
    );
    assert_eq!(
        numstat(&root, tools),
        (1, 1),
        "场景 7：SettingsTools 应恰改 1 行"
    );

    // 场景 11：TaskDetail 纯 13 行新增；新增行内容与「不碰 overflow」
    assert_eq!(
        numstat(&root, td),
        (13, 0),
        "场景 11：本票只许增 13 行（决策 215 注释 + 媒体查询），0 删除"
    );
    let mut diff = git_out(&root, &["diff", "HEAD", "--", td]);
    if diff.trim().is_empty() {
        // 提交后时序：正文取分支相对基准的 diff（三点，避开 main 侧邻接任务的改动）
        diff = git_out(&root, &["diff", "origin/main...HEAD", "--", td]);
    }
    let added: Vec<&str> = diff
        .lines()
        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
        .collect();
    assert!(
        added
            .iter()
            .any(|l| l.contains("@media (min-width: 820px) and (max-width: 1099px)")),
        "场景 11：新增行应含媒体查询"
    );
    assert!(
        added.iter().any(|l| l.contains("minmax(480px, 1fr) 280px")),
        "场景 11：新增行应含折行档列串"
    );
    assert!(
        added.iter().all(|l| !l.contains("overflow-x")),
        "场景 11：新增行不许引 overflow-x"
    );
    assert!(
        added.iter().all(|l| !l.contains("minmax(0, 1fr) 320px")),
        "场景 11：桌面档列串不许出现在改动行"
    );
    assert!(
        !diff.contains("overflow"),
        "场景 11：本票 diff 不许触碰 overflow（hero 溢出属票 01 wontfix 面）"
    );

    let changed = changed_paths(&root);
    assert!(
        !changed.iter().any(|p| p.contains("PipelineRail")),
        "场景 11：hero 轨道 PipelineRail 不在改动面"
    );

    // 场景 17 ①：改动面白名单（tests/ 一份是本测试阶段自身的硬产出，非产品改动）
    let allowed: &[&str] = &[
        "frontend/src/",
        "frontend/e2e/",
        ".scratch/ux-audit-3/IMPLEMENTATION.md",
        "tests/e2e/tests/integration/",
    ];
    assert!(!changed.is_empty(), "场景 17：改动面不应为空");
    for p in &changed {
        assert!(
            allowed.iter().any(|a| p.starts_with(a)),
            "场景 17：改动越出本票范围 → {p}"
        );
    }
    // 场景 17 ②：冻结文件不在改动面
    for f in FROZEN {
        assert!(
            !changed.iter().any(|p| p.starts_with(f)),
            "场景 17：冻结文件混进改动面 → {f}"
        );
    }

    // 场景 17 ④：四段提交 message 反查票面
    let mb = git_out(&root, &["merge-base", "HEAD", "origin/main"])
        .trim()
        .to_string();
    let log = git(&root, &["log", "--format=%s", &format!("{mb}..HEAD")]).unwrap_or_default();
    assert!(
        !log.trim().is_empty(),
        "场景 17：{mb}..HEAD 无提交——本用例要求当前检出落在审计落地那一段提交上"
    );
    for seg in [
        "票 05（ux-audit-3）",
        "票 08（ux-audit-3）",
        "票 13（ux-audit-3）",
    ] {
        assert!(
            log.contains(seg),
            "场景 17：四段 message 缺段 → {seg}\n{log}"
        );
    }
    assert!(
        log.contains("ux-audit-3"),
        "场景 17：message 无法反查票面\n{log}"
    );
}
