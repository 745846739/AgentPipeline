# 22: 前端：配置与指标页

**What to build:** 配置管理界面：providers CRUD（api_key 掩码回显）、项目管理（创建校验 git 仓库、编辑、删除保护、analyze 触发与轮询）、阶段级 agent 配置编辑（stage_configs——注意后端暂无 CRUD 端点，需要时先补端点再接 UI）、全局指标面板（成功率/阶段聚合/逃逸率/首过率）。

**Blocked by:** 20（前端应用骨架）

**Status:** ready-for-agent

- [ ] providers 页：增删改查，api_key 只显 ***，保存时不回传掩码
- [ ] projects 页：创建/编辑/删除（活跃任务删除被拒的提示）、analyze 202 + 轮询展示
- [ ] stage_configs 编辑（若补 CRUD 端点，端点属本票交付范围）
- [ ] 全局指标面板（GET /metrics）与任务级指标（GET /tasks/{id}/metrics）
