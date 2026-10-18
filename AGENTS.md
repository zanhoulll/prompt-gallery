# AGENTS.md — 项目硬规则与版本流程

## 项目上下文（开工前必读）

任何编码任务开工前先读：

- @CONTEXT.md — 术语表与领域语言定义
- @docs/adr/ — 架构决策记录；实现必须与已记录的决策一致

## 目录布局

- `src\` — Rust 源码：入口、全部 UI、主题、存储、归档、图片、测试钩子
- `build.rs` / `tests\` — 构建脚本（exe 图标与版本资源）/ 集成测试（归档格式契约）
- `docs\` — 设计文档：`adr\`、`ui-layout-map.md`、`directory-tree.md`
- `tools\` — 开发期脚本：实机测试驱动、截图、量测、数据库导出
- `AGENTS.md` / `AGENTS-RECIPES.md` / `CONTEXT.md` — 硬规则、踩坑配方、领域术语表
- `target\` — 构建产物，不入库（`cargo clean` 可重建）

运行时数据在 `%APPDATA%\PromptGallery\`（或 `PROMPT_GALLERY_DATA_DIR` 指定的位置），不在仓库里。

## 硬规则

踩坑配方按**症状**查：@AGENTS-RECIPES.md

- **落盘优先**：目标、验收标准、结论在上下文压缩前写进文件或提交（新会话先落目标，或用 `create_goal`）；需要压缩前的精确内容时先 `search_context` 再 `decompress`，大段原文用 `decompress(toFile:)` 落盘后 read。
- **文档与脚本已入库**：AGENTS.md、AGENTS-RECIPES.md、CONTEXT.md、`docs/`、`tools/` 都在版本控制里——改动用 `git diff` 对照、`git checkout -- <路径>` 回滚，不再需要手工备份；新增 / 移动 / 删除文件后同步 `docs/directory-tree.md`。
- **文档数字**：不写死体积 / 计数快照（排版、依赖、使用量一变就过期）；需要数字时按 `docs/directory-tree.md` 的「复测命令」重跑。
- **CHANGELOG 记两类账**：① **用户可感的变化**——新增 / 变更 / 修复 / 兼容性（行为、格式、版本）；`README.md` 里影响用户的说明（功能用法、**运行前置条件**等）也归这里。② **开发用文档与工具的变化**——ADR、CONTEXT、ui-layout-map、`tools/*.py` 等的新增 / 修订单列「### 文档」小节，一条写清改了什么、为什么；发布前先记在顶部 `## [Unreleased]` 暂存区，发布该版时并入并清空。`docs/directory-tree.md` 的索引职责不变。纯开发用的**功能**若用户明确说不发版本，仍不写条目。**已发布的版本条目不回改**，只约束今后新增的版本。
- **ADR 只增不改**：`docs/adr/` 已接受的决策保持原文；修正表述或标注过期内容，在标题下加带日期的注记；决策变更另开新 ADR 并回注「被 ADR-00XX 取代」。
- **收工判据**：纯函数单测覆盖边界 + 一次有效实机测试即够；同一现象失败两次就停。
- **实机测试**：优先用**应用内测试钩子**（debug 构建 + `PROMPT_GALLERY_TEST_HOOK`，`python tools/hookdrive.py --data <数据目录> --scene <场景文件>` 驱动，后台可用、每步有应答，见 ADR-0008，**每个 debug 构建恒有、与版本号无关**）。回退 Win32 时用 `tools/uicheck.ps1` 的 `-Preset` / `-Scene`，不复制它的骨架；`-OutDir` 放在 `-Data` 之外（seed 会删数据目录）。发合成输入前用 Alt 键技巧重抢前台（每个 `down` / `up` / `move` / `shot` 前各来一次），输入是否落地**以读库为准**（`python tools/dbdump.py --dir <数据目录>`），不看像素。机制见配方「合成输入无效 / 像素统计是别人的画面」。
- **截图**：见配方「截图整幅纯白 / 整幅纯黑」；每张图先判纯色，纯色即作废。
- **长按拖动**：按住类手势只用原始指针状态（`primary_pressed` / `primary_down` / `press_origin` / `press_start_time` / `total_drag_delta`）自建状态机；egui 的 `clicked` / `drag_*` / `is_pointer_button_down_on` 在按住 0.8s 后失效。详见配方「长按拖动无反应」。
- **重绘**：`row_drag` 存在期间每帧 `request_repaint()`；静止等待期没有帧，拖动虚影就不会出现。
- **布局坐标**：自绘热区 / 命中判定一律用 `ctx.content_rect()`（局部坐标，原点 = 客户区左上）；`ctx.input(|i| i.viewport().inner_rect)` 是屏幕坐标，混用会让热区整体移出窗口、点了没反应。
- **构建**：先 `Get-Process prompt-gallery | Stop-Process -Force` 再构建（残留 exe → `os error 5`，见配方）；**debug 与 release 是两份产物**——`cargo test` 与钩子只更新 `target/debug`，交给用户试之前先 `cargo build --release` 并核对时间戳，否则 exe 还是旧版本号。

## 文档地图（按触发条件读，不常驻）

- 改 UI 布局 / 控件尺寸 → @docs/ui-layout-map.md：分区结构、几何锚点、按函数名的代码落点；精确实测值按「复测」现场量，**只重测改动影响到的区域**。
- 动某个文件前（新增 / 移动 / 删除 / 判断该不该留）→ @docs/directory-tree.md：文件索引（作用 + 删除后果）、复测命令、别再补回来的路径。

## 版本流程（实现 → 独立验证 → 落账）

每版从一份临时任务书开始（规划会话产出，用完即删、不入库）；任务书里每条验收必须可执行（自动 / 文本可查 / 需实机），不可执行的不写。

1. **实现**：新会话只读该任务书与代码；编译 + 测试。**先不写** `CHANGELOG.md` 的完成条目。
2. **独立验证**：派子代理，只给文档 / 代码 / 编译产物（不给本会话摘要）；子代理只报证据（命令、输出、逐项通过 / 失败、读不懂之处），不改代码。
3. **修**：局部实现 bug 当场改；规格问题（文档错 / 漏 / 矛盾）先改文档再改代码。
4. **重验**：对失败项复跑一次——「修完了」≠「验证过了」。
5. **落账**：验证通过后才写 `CHANGELOG.md` 完成条目——它是唯一的完成真相源。
6. 换主题 → 新会话回到第 1 步。

不变量：每版都有一份「陌生人可执行」的文档；**完成 = 独立验证通过**，不是「我改完了」。
