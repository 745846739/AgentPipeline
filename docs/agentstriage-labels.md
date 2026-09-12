# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

本仓库为本地 markdown tracker，标签写在票文件的 `Status:` 行。补充两个本仓库既有状态（非 triage 角色，skills 不得混淆）：

- `done（已实现）`：票已交付并有测试锁定（见 `.scratch/agentpipeline-v1/issues/01`–`10`）
- `claimed` / `resolved`：仅用于 wayfinder 的 map/child 票

Edit the right-hand column to match whatever vocabulary you actually use.
