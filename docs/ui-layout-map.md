# Prompt Gallery — UI 布局速查

> 改 UI 时怎么用：**分区**看下面四条，**尺寸**看「几何锚点」，**具体控件**用函数名 grep「改 UI 速查」。
> 精确实测坐标**不存这里**（每改一次就过期）——需要时按文末「复测」现场量。

## 结构（客户区 1180×760 pt，原点左上，`pixels_per_point = 1.25`）

- **最上**：标题栏——左「▣ Prompt Gallery」+ 数据管理 / 关于 / 外观菜单，右三枚窗口按钮；整条可拖动、双击最大化
- **中间**：左侧栏（「+新示例」+ 示例列表，宽度可拖）+ 右侧中央区
- **中央区**：图片框在上，五分区编辑器在下（质量与基础 / 人物特征 / 服装与道具 / 姿态与动作 / 环境与光影）
- **最下**：底栏——左「保存 另存为 删除 │ 复制提示词」，右状态文案 /「草稿未保存」/ 版本号

## 几何锚点（默认 1180×760 下）

| 项 | 值 | 落点 |
| :-- | :-- | :-- |
| 窗口 | 1180×760，最小 880×560；无系统装饰，圆角 10 | `main()` |
| 标题栏 | 高 40；三枚 46×40 窗口按钮 | `title_bar()` |
| 侧栏 | 默认 248，可拖 180..420 | `sidebar()` |
| 示例行 | 高 52、行距 55（`item_spacing.y = 3`）；缩略图 40×40；锁图标占右侧 20 | `paint_sidebar_row()` |
| 图片框 | 高 = 中央可用高 × 0.42，clamp(140, 420)；与提示词框间距 6 | `image_box()` |
| 五分区 | 标题 20.4 + 文本框 29.2 + 缝线 1，一节 57.6 | `section_editor()` |
| 底栏 | 高 47.8 | `bottom_bar()` |
| 弹窗宽 | 320 草稿 / 关窗 · 340 关于 / 删除 · 560 导出 | `dialogs()` |
| 网格 | 纵 12pt / 横 6pt（12pt 汉字 = 2 列 × 1 行）；像素 ÷ 1.25 = pt | — |

## 改 UI 速查

**通则**：颜色、字号、控件尺寸一律从 `src/theme.rs` 的 `Palette` 取，`main.rs` 里不写颜色字面量。

### 窗口
- 尺寸 / 装饰 — `main()`：`with_inner_size`、`min_inner_size`、`with_decorations(false)`、`with_transparent(true)`
- 外框 — `theme.rs` `window_frame()`：1pt 描边、圆角 10（最大化 0）
- 主题 — `theme.rs` `apply()`；每帧检测 `ctx.theme()` 变化后重新应用
- 快捷键 — `handle_shortcuts()`：Ctrl+S / Ctrl+Shift+S / Ctrl+R（锁定示例上 Ctrl+R 无效）
- 八向缩放 — `resize_grips()`：8 个透明 `Area`；坐标取 `ctx.content_rect()`（局部坐标）

### 标题栏
- 面板 — `Panel::top("title_bar").exact_size(40.0)`；改动高度要同步拖动的 `Rect` 与缩放热区
- 左侧排布 — 内边距 12 → 图标 18×18 → 名称 14pt → 间距 8 → chrome 按钮（间距 6）
- chrome 按钮 — `chrome_button()`：高 28、宽 = 文字 + 20、圆角 6；**别用 `Button::frame(false)`**（连 hover 底也关掉）
- 菜单 — `Popup::menu` + `set_min_width(180)` + `menu_item()`；动作统一由 `run_action()` 执行
- 窗口按钮 — `window_button()`：46×40、贴右零间距、图形用 painter 画（不用字体图标）
- 拖动 — `interact(rect, id, Sense::click_and_drag())`；**任一子控件 hovered 时跳过**
- 左右两组控件嵌进同一个 `Layout::left_to_right` 闭包 —— 各开一个 `ui.scope` 时第二组会被换行挤到下一行并裁掉

### 侧栏
- 固定「+新示例」行 — `new_example_row()`：列表之上、不滚动、点击 = `select(None)`
- 行标题子矩形 = 「短按进编辑」命中区；锁图标独占右侧 20pt（不选中 / 不编辑 / 不起拖动）

### 中央区
- 两框比例 — `central()`：0.42 钳位 140..420；间距 `add_space(6.0)`
- 图片框整框可点选图；「×」只清引用，**不删托管文件**
- 保存时才入库 — `ingest_pending_image()`：源文件缺失报错、blake3 去重、>256MB 拒绝
- 分区数量与标题 — `model.rs` 的 `SECTION_COUNT` / `SECTION_TITLES`（只改这两处）

### 底栏
- `bottom_bar()`：保存 / 另存为 / 删除 │ 复制提示词；右侧状态文案、「草稿未保存」、版本号
- 版本号是纯文本 `Label`，**不要改回 Button**
- 导出 / 导入 / 打开数据目录不在底栏：标题栏「数据管理」菜单是唯一入口

### 弹窗
- 统一外壳 `theme.rs` `modal_shell()`；标题 `modal_title()`、说明 `modal_desc()`
- 按钮用 `primary_button` / `secondary_button` / `danger_button`，**不要用默认 `Button` 尺寸**
- Esc 在 `dialogs()` 开头统一读，各分支并入条件
- 导出弹窗 — `export_selected: BTreeSet<i64>`；列表高 260、行 32、复选框 18×18（`export_checkbox()`）

## 复测（精确实测值的唯一来源）

- 出图：`python tools/hookdrive.py --data <目录> --scene <场景>` → `<目录>/hook/shots/*.png`（客户区 1475×950）
- 量测：`python tools/imgdiff.py scan <png> --row 500 --col 700`、`color <png> 48 62 88`；**像素 ÷ 1.25 = pt**
- **增量重测**：改哪儿量哪儿；整体重测只在窗口默认尺寸变化时做

## 已知偏差（别当成 bug）

- 窗口透明 + 圆角：`PrintWindow` 抓图四角可能黑 / 白，属正常
- 动态元素（虚影、淡占位、蓝线、自动滚动热区）无法静态呈现