# 07: 兄弟文件一级展开

**What to build:** `Skill` 工具返回正文前，解析**一级**相对引用（`[text](file.md)` 形态）并内联，
使 `tdd/tests.md`、`prototype/UI.md`、`codebase-design/DEEPENING.md` 这类兄弟文件真正可用——
今天它们既不进 prompt、agent 也读不到（文件工具被锁在 worktree + 任务目录内），是**双向死指针**。

走加载器展开而**不是**放宽文件工具读根：技能根与 `{home}/data/` 同父，而 provider 密钥明文存储
（决策 112），放宽读根有读走全部 API key 的风险。

**Blocked by:** 06（`Skill` 工具——展开发生在它取正文时）

**Status:** ready-for-agent

- [ ] 解析正文里的一级相对引用并内联；**深度不递归**（对齐 Agent Skills 规范
      「Keep file references one level deep」）
- [ ] 引用目标必须是该技能目录**之内**的路径：拒绝 `../` 穿越与绝对路径
- [ ] 目标文件不存在 → `Error::Config`，报文须指出**技能名 + 缺失文件名**
- [ ] 非 `.md` 引用（如 `scripts/*.py`）**不展开**、不执行，保留原样文本（本系统无脚本执行语义，
      假装支持会让技能作者误判）
- [ ] 展开只作用于 `Skill` 工具路径；全文态注入的技能**也**应展开（否则全文态下兄弟引用仍是死指针）
- [ ] 单测：一级引用内联 / 深层引用不递归 / `../` 穿越拒绝 / 绝对路径拒绝 / 缺失文件报错带技能名 /
      非 md 引用保持原样

**Notes（实现提示）:**
- 现有上游技能的真实引用形态（用于构造 fixture）：
  `tdd` → `[tests.md](tests.md)`、`[mocking.md](mocking.md)`；
  `prototype` → `[LOGIC.md](LOGIC.md)`、`[UI.md](UI.md)`；
  `codebase-design` → `[DEEPENING.md](DEEPENING.md)`、`[DESIGN-IT-TWICE.md](DESIGN-IT-TWICE.md)`。
- 展开后的体积会膨胀（`tdd` 全包约 7KB），这是二档注入（票 05）存在的量化理由——不想要膨胀的
  技能用名字态即可。
