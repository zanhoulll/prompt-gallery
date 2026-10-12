# Prompt Gallery

_把「一张图片 + 五段正向提示词」存成一条记录，随时一键复制到 ComfyUI_

[![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-3c873a?style=flat-square)](https://www.microsoft.com/windows)
[![Rust](https://img.shields.io/badge/Rust-%3E%3D1.82-000000?style=flat-square&logo=rust)](https://rustup.rs/)
[![License](https://img.shields.io/badge/License-MIT-blue?style=flat-square)](LICENSE)
[![Binary size](https://img.shields.io/badge/binary-13.9%20MB-555?style=flat-square)](#从源码构建)

[功能](#功能) • [快速开始](#快速开始) • [使用](#使用) • [数据位置与备份](#数据位置与备份) • [开发](#开发) • [疑难排查](#疑难排查)

一个用 Rust + egui 写的 Windows 桌面工具。出图时反复使用的提示词通常散落在聊天记录、记事本和图片文件夹里；Prompt Gallery 把「一张参考图 + 对应的正向提示词」固定成一条**示例**，提示词按固定的五个**分区**组织，点一次即可复制成可直接粘贴进 ComfyUI 的一整段正文。

窗口标题与显示名称为 **Prompt Gallery**，目标是低资源占用（常驻内存约 30–50 MB）。

## 功能

- 🖼️ **图片是记录的一部分** — 点「选择图片…」或把图片拖进窗口即可入库；文件按内容的 blake3 哈希命名，同一张图重复导入不会占第二份空间
- ✍️ **五分区提示词编辑器** — 同一个滚动容器里无缝拼接的五个文本框，视觉上是一个框；中文输入法、撤销/重做、选区都可用
- 📋 **一键复制** — 只把五个分区的正文写进剪贴板，分区标题不进正文，粘贴即用
- 🗂️ **侧边栏列表** — 带缩略图，**长按约 0.2 秒后拖动**可重排顺序；短按仍然是选中，避免误触
- 💾 **草稿先编辑、后落库** — `+` 新建的示例只是草稿，点「保存」才写库并触发图片复制入库
- 📦 **整库打包** — 导出 `prompt-gallery-<时间戳>.zip`（JSON 元数据 + 图片文件），换机器导入即可恢复，且不会覆盖已有示例
- 🪶 **零外部运行时** — 单个 exe，SQLite 以 bundled 方式静态链接，不需要额外驱动

## 快速开始

### 前置条件

- **Windows 10/11 (x64)**
- **[Rust 工具链](https://rustup.rs/)** — stable，默认 host 为 `x86_64-pc-windows-msvc`（`rust-version = 1.82`）
- **MSVC 生成工具** — Visual Studio 2022 生成工具或完整版 VS（提供链接器）

### 从源码构建

```powershell
git clone <repo-url> prompt-gallery
cd prompt-gallery

# 开发构建
cargo run

# 发布构建：target\release\prompt-gallery.exe（约 13.9 MB，自包含）
cargo build --release
```

首次启动会在数据目录下自动创建数据库与托管图片目录，无需任何初始化操作。

## 使用

1. 点侧边栏的 `+` 新建示例 —— 此时它是一条**草稿**，尚未落库。
2. 填标题、选图片（或把图片拖进窗口）、按分区粘贴提示词。
3. 点「保存为新示例」—— 草稿落库，图片同时复制进托管目录。
4. 之后随时点「复制提示词」，把正文粘贴进 ComfyUI。

### 五个分区

正向提示词固定由五个分区组成，顺序与标题都不可修改：

| # | 分区标题 | 典型内容 |
| :- | :-- | :-- |
| 1 | 质量与基础 | `masterpiece, best quality, absurdres` |
| 2 | 人物特征 | `1girl, solo, silver hair, blue eyes` |
| 3 | 服装与道具 | `white shirt, black ribbon tie` |
| 4 | 姿势与动作 | `standing, hands behind back` |
| 5 | 环境与光影 | `classroom, window light, rim light` |

分区标题只是界面的分栏标记：灰色、左对齐、不可选中，复制时被剔除。

### 快捷键

| 快捷键 | 作用 |
| :-- | :-- |
| `Ctrl+S` | 保存当前示例 |
| `Ctrl+Shift+S` | 另存为副本（原示例保持不变） |
| `Ctrl+R` | 重命名 |
| `Enter`（标题框内） | 保存 |

> [!TIP]
> 复制提示词时**不会**带上分区标题，所以粘贴进 ComfyUI 的就是一段干净的逗号分隔提示词，不需要再手工删行。

## 数据位置与备份

数据库与托管图片目录整体位于数据目录下：

| 内容 | 路径 |
| :-- | :-- |
| 数据库 | `%APPDATA%\PromptGallery\gallery.sqlite3` |
| 托管图片 | `%APPDATA%\PromptGallery\images\<哈希>.<扩展名>` |

界面右上角的「打开数据目录」可直接打开该目录。

> [!IMPORTANT]
> 数据库和 `images/` 目录是配套的：手工备份时请把两者一起复制。日常备份建议直接用「导出图库…」，它会打包成一个自包含的 zip。

### 导出与导入

**导出图库…** 会写出 `prompt-gallery-<YYYYmmdd-HHmmss>.zip`：

```
prompt-gallery-20240102-030405.zip
├── data.json                 示例元数据 + 五个分区的正文（不含分区标题）
├── manifest.txt              人类可读的格式说明
└── images/
    └── <blake3 哈希>.png     图片文件，文件名即内容哈希，天然去重
```

**导入图库…** 选择导出包即可恢复。导入采用追加策略，**不会覆盖**已有示例：id 冲突的记录会自动分配新 id 并追加；缺失图片只会在状态栏提示，不阻断导入。

### 配置

数据目录可以用环境变量覆盖，便于便携部署或隔离测试：

```powershell
$env:PROMPT_GALLERY_DATA_DIR = 'D:\prompt-gallery-data'
cargo run --release
```

## 开发

```powershell
cargo test                  # 11 项单元 / 集成测试
cargo check --all-targets   # 类型检查
cargo build --release
```

测试锁住的都是「一旦回归就会静默损坏数据」的契约：分区标题不进入复制文本、图片按内容哈希去重、归档往返、id 冲突不覆盖、缩略图等比布局，以及导出包的 zip 结构。

想用真实数据做界面冒烟测试，可以复用程序自己的 `Store` 造数据（因此图片哈希命名与真实使用完全一致）：

```powershell
cargo build --bin seed
target\debug\seed.exe <数据目录> <一张图片的路径>
$env:PROMPT_GALLERY_DATA_DIR = '<数据目录>'
cargo run --release
```

### 项目结构

```
src/
├── main.rs      界面：侧边栏、图片框、五分区编辑器、弹窗、原生文件对话框
├── lib.rs       核心库模块清单（与 GUI 解耦，便于测试与工具复用）
├── model.rs     领域模型：示例（Example）、草稿（Draft）、分区（Section）
├── store.rs     数据库：SQLite schema、读写、内容哈希图片托管
├── archive.rs   导出 / 导入：JSON + 图片打包成 zip
├── images.rs    图片解码与纹理缓存（缩略图 LRU + 单张预览）
└── bin/seed.rs  开发用冒烟数据生成器
tests/
└── acceptance.rs  导出包结构契约测试
```

## 疑难排查

**启动后中文显示为方块**

程序会依次尝试 `msyh.ttc` / `msyh.ttf` / `simhei.ttf` / `simsun.ttc` 加载系统中文字体。若全部缺失（精简版系统），界面中文会退化为方块；标准 Windows 10/11 不会出现。

**界面显示「数据库无法打开」**

程序会把出错的数据目录与原因直接显示在窗口里。检查该目录的读写权限，或用 `PROMPT_GALLERY_DATA_DIR` 指到一个可写目录后重试。

**图片显示为「图片无法显示」**

托管目录中对应的图片文件缺失（例如手工删过 `images/` 里的文件，或只复制了数据库）。从导出包重新导入，或重新选择图片即可。

**导入后出现重复示例**

这是有意设计的：导入永不覆盖已有数据，id 冲突的记录会被追加为新记录。导入完成后状态栏会报告追加数与重新分配 id 的数量。

## 术语

代码与文档统一使用下列术语，改动代码或文档时请沿用，避免出现「样本」「条目」「段落」这类同义词：

| 术语 | 含义 |
| :-- | :-- |
| **示例** | 图库中的一条记录：一个标题 + 一张图片 + 五个分区的文本 |
| **当前示例** | 侧边栏选中的那一条；图片框与提示词框展示它的内容 |
| **草稿** | 通过 `+` 新建、尚未保存过的示例；保存时才落库 |
| **分区** | 正向提示词文本框内按固定标题划分的五个文本区域 |
| **分区标题** | 每个分区各自的固定标题，灰色、不可修改、不可选中，复制时被剔除 |
| **正向提示词** | 示例的提示词正文，由五个分区的文本组成 |
| **数据库** | 一个 SQLite 文件（元数据与分区文本）+ 一个托管 `images\` 目录（图片按内容哈希命名） |
