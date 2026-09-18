//! L2 集成套件（**单一二进制**，决策 218）。
//!
//! 为什么合成一个二进制：每个 `tests/*.rs` 都是一个独立的 crate root，各自要把依赖
//! 的泛型单态化一遍、再起一次 harness——本机实测这笔**每个 crate root 的固定成本约
//! 1.7–2.6 CPU-s**，而多塞一个测试文件的边际成本只有约 0.15 CPU-s。18 个文件就是
//! 把这笔固定成本付了 18 次。
//!
//! 代价与补偿：`cargo test --test <文件名>` 这种**按文件收敛编译**的能力没有了
//! （改一个文件要重编整个二进制），改由用例名前缀收敛——每个文件是一个模块，用例
//! 全名形如 `<文件模块>::<用例>`，故 `make integration TESTS=executor` 走的是
//! `-- executor::` 的名字过滤：**编译不再收敛，执行仍然收敛**。
//!
//! 加新测试文件：把文件放进本目录，并在下面加一行 `mod <文件名>;`。

mod conversation_archive;
mod cursor_lifecycle;
mod egress;
mod env_mode;
mod executor;
mod foreman;
mod foreman_proposals;
mod foreman_sessions_migration;
mod git_chain;
mod llm_smoke;
mod pairing;
mod process_group;
mod production_llm;
mod project_analysis_observation;
mod repair;
mod repo_live;
mod scheduler_tick;
mod server_bind;
