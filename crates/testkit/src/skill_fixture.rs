//! 技能目录与技能包的 fixture（决策 172⑤，票 09）。
//!
//! 票 09 的三条入口（上传 zip / 目录导入 / 扫描）都需要「一个像样的技能包」当输入，票 11 的
//! 预览与票 15 的界面也各要一份。与其在每个测试文件里重写一遍，不如在这里收成两个函数：
//! [`write_skill_dir`]（真目录，给目录导入与扫描用）与 [`zip_bytes`]（zip 字节，给上传用）。
//!
//! **不引入新的可测试性接缝**（决策 143）：本模块只是造输入数据的助手，不替换任何生产实现。

use std::io::Write;
use std::path::{Path, PathBuf};

/// 在 `parent/{name}/` 下写一个技能目录，返回该目录路径。
///
/// `body` 是 `SKILL.md` 的正文（自动补 frontmatter `name:`——与目录名一致，因为名字是唯一
/// 身份，不一致会被导入拒绝）。`siblings` 是额外的兄弟文件（相对路径 → 内容），
/// 用于验证票 07 的兄弟文件随技能一并落盘。
pub fn write_skill_dir(
    parent: &Path,
    name: &str,
    body: &str,
    siblings: &[(&str, &str)],
) -> PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\n---\n\n{body}\n"),
    )
    .unwrap();
    for (rel, content) in siblings {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    dir
}

/// 写一个技能目录，但 `SKILL.md` 的内容**逐字给定**（不走 `write_skill_dir` 的 frontmatter 包装）。
///
/// 用于构造非法样本（空正文、frontmatter `name` 与目录名不符、坏 frontmatter…）。
pub fn write_raw_skill_dir(parent: &Path, name: &str, raw_skill_md: &str) -> PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("SKILL.md"), raw_skill_md).unwrap();
    dir
}

/// 把 `(相对路径, 内容)` 打成 zip 字节（票 09 的上传入口用）。
///
/// 入口名**原样写入**——包括 `../evil.md` 这类恶意写法，因为本函数的用途正是构造这类样本
/// 来验证导入端的拒绝逻辑（`zip` crate 的 writer 不做路径校验，与真实攻击面一致）。
pub fn zip_bytes(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut buf);
        let opts: zip::write::SimpleFileOptions = Default::default();
        for (name, content) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(content.as_bytes()).unwrap();
        }
        w.finish().unwrap();
    }
    buf.into_inner()
}

/// 单技能包的 zip（`{name}/SKILL.md` + 兄弟文件），即 `zip -r skill.zip skill/` 的形态。
pub fn skill_zip(name: &str, body: &str, siblings: &[(&str, &str)]) -> Vec<u8> {
    let mut entries: Vec<(String, String)> = vec![(
        format!("{name}/SKILL.md"),
        format!("---\nname: {name}\n---\n\n{body}\n"),
    )];
    for (rel, content) in siblings {
        entries.push((format!("{name}/{rel}"), (*content).to_string()));
    }
    let refs: Vec<(&str, &str)> = entries
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    zip_bytes(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_dir_has_skill_md_and_siblings() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = write_skill_dir(tmp.path(), "grill", "正文", &[("tests.md", "兄弟")]);
        assert!(dir.join("SKILL.md").is_file());
        assert!(dir.join("tests.md").is_file());
        let raw = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
        assert!(raw.contains("name: grill"), "{raw}");
        assert!(raw.contains("正文"), "{raw}");
    }

    #[test]
    fn skill_zip_round_trips_through_the_reader() {
        let bytes = skill_zip("grill", "正文", &[("tests.md", "兄弟")]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| archive.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.contains(&"grill/SKILL.md".to_string()), "{names:?}");
        assert!(names.contains(&"grill/tests.md".to_string()), "{names:?}");
    }

    #[test]
    fn raw_zip_preserves_hostile_entry_names() {
        let bytes = zip_bytes(&[("../evil.md", "x")]);
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(archive.by_index(0).unwrap().name(), "../evil.md");
    }
}
