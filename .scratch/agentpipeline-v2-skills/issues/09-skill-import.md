# 09: 技能导入（本地 + 目录扫描）

**What to build:** 用户能装技能：① **上传 zip / 目录** → 校验结构（须含 `SKILL.md`）与 frontmatter
→ 落到技能根；② **扫描一个本地技能根**（典型如 `~/.zcode/skills`）批量导入，逐个显示预览。
同名冲突**默认拒绝**并报出已存在的来源，覆盖需显式确认。

**Blocked by:** 02（frontmatter 解析——导入时要校验并读取描述）

**Status:** done

- [x] 上传 zip / 目录 → 校验含 `SKILL.md`、frontmatter 可解析、正文非空；不合法则拒绝并说明原因
- [x] 导入落到技能根（受 `[skills] dir` 影响，票 01），布局为 `{name}/SKILL.md` + 兄弟文件
- [x] 同名冲突默认拒绝，错误报文指出冲突技能名与其当前来源；显式确认后才覆盖
- [x] 目录扫描：列出一个本地技能根下的全部技能，返回名字 + 描述 + 是否已存在
- [x] 批量导入可逐个成功/失败，不因一个坏技能中断整批（逐项结果返回）
- [x] 卸载技能；**卸载仍被阶段配置引用的技能**时，引用它的配置在下一次启动校验 / `PUT /stage-configs`
      时 fail fast（技能名是唯一身份，不得静默降级）
- [x] API 契约用例：上传合法 zip 成功 / 缺 `SKILL.md` 拒绝 / 同名未确认拒绝 / 扫描返回清单 / 卸载后引用报错
- [x] 本机无网时全部功能可用（本票不依赖任何网络）

**实现结论：**
- 新模块 `crates/core/src/agent/skill_import.rs`（技能包 + 校验 + 落盘 + 扫描 + 卸载）与
  `crates/app/src/routes/skills.rs`（五个端点）。zip 与目录两条入口在 `SkillPackage` 收口，
  故校验 / 冲突 / 落盘只实现一次；票 10 的 registry 复用 `SkillPackage::from_zip`。
- **新依赖 `zip` 4.2**（`default-features = false` + `deflate-flate2-zlib-rs`）：只读不写，
  故关掉 aes/bzip2/zstd/lzma/ppmd；`zlib-rs` 后端纯 Rust，不引 C 工具链。给 testkit 也加了它
  （造 zip fixture）。落进锁文件的新包：zip / flate2 / zlib-rs / crc32fast / arbitrary / derive_arbitrary。
- **路径穿越的两道判定不合并**：自己的 `sanitize_rel_path`（报「绝对路径」「含 `..` 穿越」这类精确原因）
  + `zip` crate 的 `enclosed_name`（该 crate 的历史漏洞 GHSA-94vh-gphv-8pm8 正是「规范化不当导致任意写」）。
  落盘前再逐级 `canonicalize` 确认目标在技能根之内。**咬合检查做过**：关掉自己那道，
  `zip_absolute_path_is_rejected` / `zip_parent_traversal_is_rejected` 立刻失败。
- **卸载不检查引用**（设计决定，非遗漏）：把引用检查塞进卸载会制造「想卸载得先改配置、想改配置得先
  卸载」的隐蔽先后依赖。引用完整性仍由启动校验与 `PUT /stage-configs` fail fast 兜住，
  另在 `GET /skills` 的 `declared_in` 提前告知后果。
- **新增 testkit `skill_fixture`**（`write_skill_dir` / `write_raw_skill_dir` / `zip_bytes` / `skill_zip`）：
  票 11 的预览与票 15 的界面也要造同样的输入，避免各测试文件重写一遍。**不新增可测试性接缝**（决策 143）。
- **契约面**：`core::config::declared_skill_decls`（宽容版，供只读界面路径用——坏配置不该让
  「哪条配置坏了」的查询本身失败）。
- **顺带覆盖**：macOS 打包噪声（`__MACOSX/` / `.DS_Store`）剔除；解压炸弹防护（单条目 16 MiB、
  条目数 4096、读取时 `take` 截断）；zip 两种真实布局（带一层目录 / 平铺，后者需显式给名）。
- **明确不做**（票 11/10 的边界）：装前预览与信任标记、远程 registry、签名与审核队列。

**code-review 收口（Spec 轴抓到的三个真缺陷，均已修并补用例）：**
1. **名字含 `/` 时「装得进、列不出、删不掉」**——`from_entries_with_name` 沿用了条目路径的判定
   （允许 `a/b`），于是 `{root}/a/b/SKILL.md` 写得下去，但 `discover` 只扫技能根下一层（列不出来）、
   `uninstall` 又拒绝含分隔符的名字（删不掉）——于用户是静默失效。修法：新增 `sanitize_skill_name`
   （在 `sanitize_rel_path` 之上加「不得含路径分隔符」），**三个入口共用同一条名字不变量**。
   顺带把「深层嵌套的包」从含糊的「不含 `SKILL.md`」改成明确的「目录层级过深」。
2. **`overwrite=true` 会静默遮住 PATH 工具型技能**——冲突检查原本只在 `!overwrite` 时生效，
   而删除只作用于技能根里的那份，PATH 文件删不掉：装了同名 markdown 只会把工具型技能**遮住**
   （知识型优先），用户以为覆盖了、其实那份还在。修法：工具型冲突**不受 `overwrite` 影响**，
   一律 409 并要求改名。
3. **`declared_skill_decls` 会因一条坏声明丢掉整份配置**——早先是整份 `unwrap_or_default()`，
   于是一条坏的节点声明会让同阶段**其他节点**的技能也报成「无人引用」，恰好在「卸载前看后果」
   这个用途上给出**错误的安全感**。修法：拆出 `declared_skill_sources`（逐来源各自一个 `Result`），
   宽容版**只丢坏的那一处**；严格版（写入 / 启动路径）行为不变，仍 fail fast。

**未采纳的 Spec 意见（记录理由）：** 「目录导入的来源路径也要拒绝 `../` 与绝对路径」——票面那句
指的是**包内条目**的语义，而导入来源本身**必须**能是绝对路径（`~/.zcode/skills/foo` 就是主用例），
拒绝它会直接废掉票 09 的目录导入。真正要守的不变量是「**派生出的技能名**不能逃出技能根」，
这一条已由缺陷 1 的 `sanitize_skill_name` + 落盘的 `ensure_inside` realpath 校验共同兜住。

**Standards 轴**（同批自查）：`zip` 依赖关掉默认特性（只读不写）、`ExistingSource` 收掉重复的
`discover` 调用、删掉无用的 `read_description` / `skill_md_path` 两个死函数。


**Notes（实现提示）:**
- 路径穿越是这里的主要风险：zip 内条目名与导入来源路径都须拒绝 `../` 与绝对路径，落盘前
  realpath 校验目标在技能根之内（与 `FileToolPolicy` 同口径的判定思路）。
- 兄弟文件随技能目录一并落盘——否则票 07 的展开会缺文件。
