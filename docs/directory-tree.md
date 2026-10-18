# 项目目录树（directory-tree）

回答两个问题：**每个文件干什么、删了会怎样**。目录结构不抄进来——`git ls-files` 是唯一真相；`.gitignore` 只挡 `target/`、`data/`、`*.zip` 与编辑器杂物。

- 体积 / 计数 / 耗时一律不写死，需要数字时跑文末「复测命令」。

## 文件索引

| 路径 | 作用 | 删了会怎样 |
| :-- | :-- | :-- |
| `src\main.rs` | 应用入口 + 全部 UI：侧栏、图片框、五分区编辑器、自绘标题栏、弹窗、原生文件对话框；CJK 字体回退 | 界面全丢，等于重写 |
| `src\testhook.rs` | 应用内测试钩子（debug-only，`PROMPT_GALLERY_TEST_HOOK` 门控）：注入输入、GPU 回读截图、文件 IPC | `tools/hookdrive.py` 的实机矩阵失效；发布产物不受影响 |
| `src\theme.rs` | 颜色与控件尺寸的唯一来源（`Palette` 深/浅两套）、spacing、弹窗外壳、按钮样式、`config.json` | 界面退回 egui 默认样式，外观设置也无从读取 |
| `src\icon_render.rs` | 图标像素：256 设计稿渲染 + `.ico` 打包（纯计算，`build.rs` 复用） | 窗口、标题栏、exe 三处图标一起消失 |
| `src\icon.rs` | egui 侧包装：纹理缓存 + `paint()` | 编译失败 |
| `src\lib.rs` | 库 crate 入口，声明 model / store / archive / images | 编译失败 |
| `src\model.rs` | 领域模型与五分区标题常量（增删分区只改这里） | 分区契约要重写 |
| `src\store.rs` | SQLite schema、增删改查、按内容哈希托管图片、数据目录解析 | 已有图库失去唯一的读取实现 |
| `src\archive.rs` | 导出 / 导入 zip（JSON 元数据 + 图片，id 冲突不覆盖） | 备份与恢复功能丢失 |
| `src\images.rs` | 解码、等比缩放、缩略图 LRU 与单张预览缓存 | 图片显示不出来 |
| `src\bin\seed.rs` | 开发用冒烟数据生成器（复用 `Store`） | README 的冒烟步骤失效 |
| `src\bin\iconasset.rs` | 开发用：`cargo run --bin iconasset` 刷新 `docs\images\icon.png` / `icon.ico` | 文档图标停在旧版 |
| `build.rs` | 构建脚本（不参与运行）：`icon_render` → `.ico` + VERSIONINFO，经 `rc.exe` 链进 exe | exe 失去图标与版本信息，资源管理器里是默认图标 |
| `tests\acceptance.rs` | 归档格式契约：`data.json` 字段、`images\` 命名、分区标题不入正文 | 改导出格式时容易静默破坏兼容性 |
| `Cargo.toml` / `Cargo.lock` | 依赖声明 / 精确版本锁定（可重现构建） | 无法构建；或依赖漂移，行为与体积都变 |
| `AGENTS.md` | 硬规则、文档地图、版本流程（AI 助手的输入） | 约定丢失，后续改动容易偏离既有风格 |
| `AGENTS-RECIPES.md` | 按症状查的踩坑配方 | 同类坑重踩，验证脚本用法要重新发现 |
| `CONTEXT.md` | 领域术语表：示例 / 草稿 / 分区 / 锁定 | 代码与文档出现同义词漂移 |
| `docs\adr\*.md` | 架构决策记录（ADR）：每个重要取舍一份，编号递增 | 「为什么这样选」无从追溯 |
| `docs\ui-layout-map.md` | UI 布局图：分区结构 + 几何锚点 + 按函数名的代码落点 | 改 UI 时失去对照，得从零重测 |
| `docs\images\*` | 文档用图标（`icon.png` / `icon.ico`），**不是运行时资源** | `cargo run --bin iconasset` 重新生成 |
| `tools\*.py` / `uicheck.ps1` | 验证脚本：钩子驱动、截图、像素量测、查库、字形核对 | 实机验证要重新造轮子 |
| `target\` | Cargo 构建输出，全部可由源码重建 | 下次构建数分钟到十几分钟；发布 exe 要先复制出去 |

## 保留清单（不再开发时）

| 保留 | 为什么 |
| :-- | :-- |
| 仓库外的 `prompt-gallery.exe` | 唯一能直接运行的程序本体 |
| `src\` + `Cargo.toml` + `Cargo.lock` | 唯一能重建它的创作输入 |
| `%APPDATA%\PromptGallery\` | 唯一不可再生的图库数据 |

## 产物与运行数据边界

- `target\` 整目录可 `cargo clean`。发布物自包含：单独复制到空目录、仓库外都能跑，不读同目录任何文件。
- 跑起来仍依赖系统：DX12 / Vulkan / OpenGL、VC++ 运行库、中文字体（`msyh.ttc` 等）、可写的数据目录。
- 图库数据换机器 / 换用户看到的是空图库（数据目录位置与内容见 README）。

## 复测命令（要数字时再跑）

- `cargo clean --dry-run` — 删除 `target\` 的文件数与总量
- `Get-Item target/release/prompt-gallery.exe | Select-Object Length` — 发布物字节数
- `Get-ChildItem $env:APPDATA\PromptGallery -Recurse -File | Measure-Object Length -Sum` — 图库数据体量

## 别再补回来的路径

- `docs\设计取舍.txt` — 早期清单里的占位，从未存在过（曾在本文件里被引用）。
- `data\` — 只在用 `PROMPT_GALLERY_DATA_DIR` 把数据目录指进仓库时出现；默认不在项目里。
- `src\main.rs` 里的 `window_title()`、侧栏悬停铅笔 ✎ — v0.1.3 已删；标题栏改由 `title_bar()` 自绘（ADR-0009）。