# Prompt Gallery — UI 布局速查

> 改 UI 时怎么用：**分区**看下面四条，**尺寸**看「几何锚点」，**具体控件**用函数名 grep「改 UI 速查」。
> 精确实测坐标**不存这里**（每改一次就过期）——需要时按文末「复测」现场量。

## 结构（客户区 1180×760 pt，原点左上，`pixels_per_point = 1.25`）

- **最上**：标题栏——左「▣ Prompt Gallery」+ 数据管理 / 关于 / 外观菜单，右三枚窗口按钮；整条可拖动、双击最大化
- **中间**：左侧栏（「+新示例」+ 示例列表，宽度可拖）+ 右侧中央区
- **中央区**：按图片朝向自适应（ADR-0011 起只有正/负向两个文本框）——横图：图片在上占满整行，下方正向在左、负向在右；竖图：图片在左，右列上正向、下负向
- **最下**：底栏——左「保存 删除」，右状态文案 /「草稿未保存」/ 版本号

## 几何锚点（默认 1180×760 下）

| 项 | 值 | 落点 |
| :-- | :-- | :-- |
| 窗口 | 1180×760，最小 880×560；无系统装饰，圆角 10 | `main()` |
| 标题栏 | 高 40；三枚 46×40 窗口按钮 | `title_bar()` |
| 侧栏 | 默认 248，可拖 180..420 | `sidebar()` |
| 示例行 | 高 52、行距 55（`item_spacing.y = 3`）；缩略图 40×40；锁图标占右侧 20 | `paint_sidebar_row()` |
| 中央区排版 | 常量 `CENTRAL_GAP = 6`、`IMAGE_SHARE = 0.42`、`POSITIVE_SHARE = 0.62`；退化为横版的下限 `MIN_IMAGE_COL_W = 240`、`MIN_TEXT_COL_W = 320` | `central_layout()` |
| 横版（宽高比 ≥ 1 或无图） | 图片占满整行，高 = 可用高 × 0.42，clamp(140, 420)；余下横带按宽度 62 : 38 左右分成正向 / 负向，两者同高 | `central()`、`image_box()` |
| 竖版（宽高比 < 1） | 图片列宽 = 可用宽 × 0.42（下限 240）、满高；文本列吃剩余宽（减 6）；文本列内 62 : 38 上下分 | `central()`、`prompt_box()` |
| 提示词框 | 外框圆角 6、1pt 描边；顶部 6pt：锁字形 12pt + 小标题 11pt；右上角复制按钮 44×18（右 8 / 上 6，文字 12pt：常态 muted、悬停提亮、空文本降低对比）；正文顶内缩 = 6 + 18 + 6 | `prompt_box()` |
| 底栏 | 高 47.8 | `bottom_bar()` |
| 弹窗宽 | 320 草稿 / 关窗 · 340 关于 / 删除 · 420 覆盖提示词 · 560 导出 | `dialogs()` |
| 网格 | 纵 12pt / 横 6pt（12pt 汉字 = 2 列 × 1 行）；像素 ÷ 1.25 = pt | — |

## 改 UI 速查

**通则**：颜色、字号、控件尺寸一律从 `src/theme.rs` 的 `Palette` 取，`main.rs` 里不写颜色字面量。

### 窗口
- 尺寸 / 装饰 — `main()`：`with_inner_size`、`min_inner_size`、`with_decorations(false)`、`with_transparent(true)`
- 外框 — `theme.rs` `window_frame()`：1pt 描边、圆角 10（最大化 0）
- 主题 — `theme.rs` `apply()`；每帧检测 `ctx.theme()` 变化后重新应用
- 快捷键 — `handle_shortcuts()`：Ctrl+S / Ctrl+R（锁定示例上 Ctrl+R 无效）
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
- 锁字形颜色：普通锁 `text_muted`；来源锁（`prompt_source = image`，ADR-0012）用 `accent`，点击只弹说明、不解锁。来源锁不拦标题内联改名与「删除」按钮（只锁内容），普通锁仍拦

### 中央区
- 排版 — `central()` 先取 `cached_image_aspect()`，再调纯函数 `central_layout(avail, aspect)` 得到三块矩形（有单测）；改比例只动上面那组常量
- 图片框整框可点选图；「×」只清引用，**不删托管文件**
- 保存时才入库 — `ingest_pending_image()`：源文件缺失报错、blake3 去重、>256MB 拒绝
- 选图即检测 ComfyUI 提示词 — `set_pending_image()` → 需要覆盖时挂 `prompt_confirm` → `commit_image_choice()`（ADR-0013）
- 提示词框 — `prompt_box(ui, rect, kind)`：`PromptKind::{Positive, Negative}` 决定标题、滚动区盐值与自动保存的焦点 id

### 底栏
- `bottom_bar()`：保存 / 删除；右侧状态文案、「草稿未保存」、版本号
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