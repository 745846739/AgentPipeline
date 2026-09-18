//! L3 API 契约 + 绑定 / 市场 / 冒烟的集成套件（**单一二进制**，决策 218）。
//!
//! 合成一个二进制的原因与代价见 `crates/core/tests/integration/main.rs` 的文件头。
//! 分层子集改用例名前缀收敛：本目录每个文件是一个模块，用例全名形如
//! `<文件模块>::<用例>`，故 `make api` 走 `-- api_contract::`、
//! `make smoke` 走 `-- smoke::`——编译不再按层收敛，执行仍然按层收敛。

mod api_contract;
mod lan_bind;
mod market;
mod port_stability;
mod restart_recovery;
mod smoke;
