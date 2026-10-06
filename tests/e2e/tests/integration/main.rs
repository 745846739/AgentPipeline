//! L4 端到端场景（**单一二进制**，决策 218）。
//!
//! 合成一个二进制的原因与代价见 `crates/core/tests/integration/main.rs` 的文件头。
//! 本目录每个场景文件是一个模块、共用 `common`；用例全名形如
//! `<场景模块>::<用例>`，故 `make e2e TESTS=gates` 走 `-- gates::`。
//!
//! 注意：子模块里引用共享夹具要用 `super::common::…`（原先各文件是 crate root，
//! 所以写的是裸 `common::…`）。加新场景：放文件进本目录 + 在下面加一行 `mod`。

mod common;

mod conflicts;
mod crash_recovery;
mod deps;
mod gates;
mod happy_path;
mod join_and_skip;
mod pending;
mod reviews;
mod timeouts;
mod ux_audit3_artifacts;
