# 更新日志

Prompt Gallery 的所有重要变更都记录在此。
格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循[语义化版本](https://semver.org/lang/zh-CN/)。

## [v0.1.3] - 2026-10-16

一次「让改内容像重命名一样自动落库」+ **示例只读锁**，外加三处界面瑕疵清理：选中行悬停时的铅笔豆腐块、`+新示例` 未居中、标题栏冗长。

### 新增

- **已落库示例自动保存（Autosave）**——标题与五个分区的改动在停止输入 **500ms** 后自动写库；切换示例、文本框失焦、关窗、导出、上锁前强制 flush（失败则取消本次切换 / 关窗 / 上锁并报错）。图片的选入 / 拖入 / 移除**立即写库**。因此已落库示例之间切换**不再弹确认框**；写入平时静默、不弹「已保存」。**草稿不改语义**：仍点「保存为新示例」才落库。（ADR-0006）
- **示例只读锁（Locked）**——侧边栏每行右端的锁图标：已锁常显 🔒、未锁悬停显 🔓、草稿不显示；图标独占命中区，点一下就切换并即时写库、无二次确认。锁定后标题 / 图片 / 五分区 / 删除全部禁改（底栏「保存」「删除」置灰、五分区灰化只读、图片框点击与拖入被拒、`Ctrl+R` 改名无效），但**拖动排序、选中查看、复制提示词、另存为副本仍然可用**。约束同时在数据库层强制：`Store::update` / `Store::delete` 对 `locked = true` 的行直接返回错误，界面置灰只是表现。（ADR-0007）
- **旧数据库首次打开自动升级到 schema v2**——migrate 改为三分支：新库建表即带 `locked` 列；v1 旧库执行一次 `ALTER TABLE examples ADD COLUMN locked INTEGER NOT NULL DEFAULT 0`；v2 及以上直接返回。已有示例原样保留并全部视为未锁。

### 变更

- **已落库示例改为选图 / 拖图即入库**——不再等待某个「保存时刻」，选图立刻按 blake3 哈希复制进托管 `images/` 并写 `image_hash`；**草稿仍走 `PendingImage`（保存时才入库）**。此变更**部分取代 ADR-0004**，ADR-0004 已加注记。
- **窗口标题栏恒为 `Prompt Gallery`**——删除 `window_title()` 与 `ViewportCommand::Title`，示例名与 `*` 脏标记不再进入标题栏；「未保存」信号改为底栏版本号左侧的「**草稿未保存**」，仅在当前是**非空草稿**时显示。（`src/main.rs`）
- **`+新示例` 文案改为水平 + 垂直居中**——按 `rect.center()` 排版，拉宽侧边栏后仍居中；悬停底色与提示文案不变。（`new_example_row()`）
- **侧栏行标题改为上下居中**，并为右端锁图标预留 20pt「图标道」（换行宽度收为 `text_rect.width() − 20pt`），避免未锁行悬停出现 🔓 时标题重排跳动。
- **版本号** `0.1.2` → `0.1.3`。（`Cargo.toml`）

### 修复

- **选中示例悬停时行右端出现小方块**——那是字面 `✎`（U+270E）在 Ubuntu-Light / NotoEmoji / emoji-icon-font / 微软雅黑 上均缺字形而渲染出的豆腐块。直接删除该绘制，不换字形；标题悬停的 I-beam 光标与内联编辑（短按标题 / `Ctrl+R`）保持原样。

### 测试

- 新增 6 个单测：`src/model.rs` 的 `locked_defaults_false_and_is_not_part_of_content_diff`；`src/store.rs` 的 `new_database_has_locked_column_and_version_2`、`old_database_migrates_to_v2_keeping_rows_unlocked`、`locked_rows_reject_update_and_delete_but_reorder_and_unlock_work`；`src/archive.rs` 的 `export_writes_locked_and_old_json_defaults_to_unlocked`、`locked_survives_export_import_roundtrip`。
- `cargo test` 全绿：核心库 11 → **17** + `src/main.rs` 19 + 验收 1 = **37 项通过**，零 warning；`cargo check --all-targets` 通过。

### 兼容性

- 数据库 schema（`SCHEMA_VERSION`）`1` → `2`：旧库首次打开自动加列并置 `user_version = 2`，已有数据不动、全部未锁。
- 归档格式（`ARCHIVE_VERSION`）**仍为 `1`**：导出 JSON 多一个可选 `locked` 字段——新版读旧包 = 全部未锁，旧版读新包 = 忽略该字段，双向不崩。

## [v0.1.2] - 2026-10-14

一次**交互根因修复** + 侧边栏入口重排：长按拖动改为**从指针物理按下的瞬间计时**（此前按住不动永远无法进入拖动）；拖动新增**整行虚影**与**边缘自动滚动**；侧边栏顶部的「示例 N」计数区换成**固定的 `+新示例` 行**。

### 新增

- **拖动虚影（Drag Ghost）**——长按达标（280ms）的瞬间，被拖行以约 85% 不透明度的整行副本（缩略图 + 标题 + 该行自身底色）上浮 2–4pt 并淡入，之后 1:1 跟手；源行原位保留约 35% 不透明度的淡占位，列表不塌陷；虚影画在 `Order::Foreground` 独立图层，不被 `ScrollArea` 裁剪、盖住相邻行；插入蓝线 2pt `rgb(120,170,255)` 仍画在虚影下层。`paint_sidebar_row` 新增 `opacity` / `elevated` 参数，虚影与源行淡占位复用同一套绘制。
- **边缘自动滚动**——虚影已出的前提下，指针进入列表视口上下各 20pt 的热区即持续滚动：内缘约 4pt/帧、贴边约 16pt/帧（60fps ≈ 240–960pt/s）；指针静止在热区也继续滚，离开热区即停，滚到首/尾即停、不越界不循环；拖动期间每帧重算落点。
- **侧边栏顶部固定的 `+新示例` 行**——始终可见、不随列表滚动、不可拖动、不参与落点计算、永不高亮为选中蓝；与普通行同尺寸但不显示缩略图，文案左对齐，悬停提示「新建示例（草稿）：保存时才落库」；空列表也始终显示。

### 变更

- **长按判定改为从指针物理按下计时（根因修复）**——旧实现把计时锚点放在 `response.drag_started()` 上（egui 判定「开始拖动」需要先有位移），于是**按住不动时计时器根本不启动**，且落点只在 `response.dragged()` 门控下才计算；按住超过 0.8s 后 `response.clicked()` 也不成立，松手后毫无反应。现在改用 `i.pointer.press_start_time()` 作锚点，并自建手势状态机（`Waiting` / `Armed` / `Cancelled`）：达标前累计位移超过 6pt 即取消本次长按，达标后位移超过 4pt 才进入「重排中」。`REORDER_HOLD` 保持 280ms、`REORDER_SLOP` 保持 4pt。（ADR-0005）
- **松手语义收紧**——原地松手与空操作（`reorder_destination` 返回 `None`）只做「选中」，不再调用 `move_row`；达标前位移超限的手势既不重排也不选中。
- **拖动与内联编辑互斥**——标题正在内联编辑的行不允许发起拖动，其它行不受影响。
- **删除侧边栏「示例 N」计数区**——连同其 `ui.separator()` 整块移除，列表从侧边栏顶部开始。
- **版本号** `0.1.1` → `0.1.2`。（`Cargo.toml`）

### 测试

- 新增 6 个单测锁住手势状态机与自动滚动：`long_press_cancels_when_moved_before_hold`、`long_press_arms_at_hold_even_without_movement`、`reorder_needs_slop_after_arming`、`release_outcomes_follow_the_gesture_phase`、`autoscroll_speed_curve_and_direction`、`autoscroll_speed_matches_the_two_reference_points`。
- 拖拽落点测试把原 `drag_insertion_index_keeps_below_list_means_end` 用例**替换**为越界语义用例 `drag_insertion_index_clamps_outside_the_viewport`（指针在视口上方一律第 0 位、下方一律末尾，即使列表已滚动）——是替换不是净增，`src/main.rs` 测试数 13 → 19 全部来自上面的 6 个新单测。
- `cargo test` 全绿：核心库 11 + `src/main.rs` 19 + 验收 1 = **31 项通过**，零 warning。

### 文档

- `README.md`：长按措辞改为「按住不动约 0.28 秒」（达标前移动会取消），补固定 `+新示例` 行与边缘自动滚动，版本号与测试项数同步更新。

### 兼容性

- 数据库 schema（`SCHEMA_VERSION = 1`）与归档格式（`ARCHIVE_VERSION = 1`）**均未改变**，0.1.1 的数据目录与导出包可直接使用。

## [v0.1.1] - 2026-10-14

一次界面重构：**顶栏整块删除**，选图并入图片框、改名并入侧边栏；四个图库按钮移到**底栏**；图片改为**保存时才入库**；**草稿成为侧边栏置顶的一行**。

### 新增

- **图片框整框可点选图**——图片框由 `Sense::hover()` 改为 `Sense::click()`，有无图片都打开原生文件对话框；悬停描边高亮 + 手型光标；已有图片时叠加半透明「点击更换图片」遮罩；文件拖入窗口时描边变 `rgb(120,170,255)`。占位文案改为「点击或拖拽本地图片到此处」。（`src/main.rs:982 · 1044-1051 · 1115-1140`）
- **图片框右上角「×」移除图片**——悬停时出现在图片框 `(right−26, top+6)`，只解除引用，不删除托管目录中的文件。（`src/main.rs:1013-1033`）
- **侧边栏标题内联编辑**——`Ctrl+R` 或短按**已选中行**的标题进入编辑；Enter / 失焦确认，Esc 取消；已落库示例立即写库，草稿只改内存；当前行悬停时标题右侧出现铅笔 `✎`、光标变 I-beam。新增 `begin_inline_edit()` / `commit_inline_edit()` 与 `sidebar_row_text_rect()`（标题子矩形同时用于绘制与命中判定，不影响从标题上开始的长按拖动）。（`src/main.rs:762 · 777-812 · 1555-1561`）
- **草稿行置顶**——`+` 新建的草稿固定显示在列表最上、带「草稿」标记、不可拖动排序、标题可内联编辑；保存后仍留在顶部。（`src/main.rs:566-583 · 1536-1543`）
- **底栏版本按钮与「关于」弹窗**——最右一个纯文本按钮 `v0.1.1`（取自 `env!("CARGO_PKG_VERSION")`，hover 提示「关于 Prompt Gallery」）；点击弹出 380pt 小窗，显示版本、数据目录完整路径、「复制路径」/「打开数据目录」与归档格式版本。（`src/main.rs:900-915 · 1207-1240`）

### 变更

- **删除顶栏**——顶部工具条整块移除（原「选择图片…」「移除图片」「标题输入框」「复制提示词」「导出/导入/打开数据目录」及删除按钮），内容区上移约 38pt；示例标题仅保留在窗口标题栏，有未保存改动时前缀 `*`。（`src/main.rs:476-487`）
- **底栏重排**——顺序固定为：`保存`（草稿时文案「保存为新示例」）`另存为` `删除` │ `复制提示词` `导出图库…` `导入图库…` `打开数据目录` │ 状态文案 ……（最右）`v0.1.1`；分组之间为 `ui.separator()`；状态文案用 `Label::truncate()` 自适应截断，最小窗口宽 880pt 下按钮不换行、不隐藏；「复制提示词」沿用五分区正文全空时置灰的逻辑。（`src/main.rs:842-925`）
- **图片改为保存时才入库**——选图 / 拖图只记住源文件路径（`PendingImage`），点「保存」时才读取内容并按 blake3 哈希写入托管目录；源文件在保存前被移动或删除会**报错并提示重新选择**，不静默丢图；超过 256MB 拒绝；「另存为」时已在托管目录的同内容文件直接复用，不重复写。（`src/main.rs:344-382 · 419-436`、`src/store.rs`）
- **拖拽取源路径而非字节**——窗口级 `dropped_files` 改用 `file.path()`。（`src/main.rs:443-467`）
- **长按拖动时长 200ms → 280ms**——`REORDER_HOLD` 调整为 280ms，`REORDER_SLOP` 保持 4pt。（`src/main.rs:44-46`）
- **草稿落库位置**——草稿走新增的 `Store::insert_at_top()`，占 `position 0` 并将其余示例整体后移；导入归档与开发用 seed 仍走 `insert` / `insert_with_id` **追加到末尾**，这是有意的不对称。（`src/store.rs:159`）
- **侧边栏「示例 N」计数**只统计已落库示例，不含草稿行。（`src/main.rs:543-547`）
- **版本号** `0.1.0` → `0.1.1`。（`Cargo.toml`）

### 修复

- **长按拖动无法改变顺序**（v0.1.1 之前的 bug）——落点原先在遍历循环内、被 `response.dragged()` 门控，只能取到被拖行自己的矩形与序号，两个方向都被 `reorder_destination` 抵消成空操作（蓝线照画、松手无变化）。改为循环内只收集各行矩形，循环结束后用纯函数统一求落点；指示线改画在真实落点并裁到滚动视口内；换算数学本身未变，只是输入换成了真实落点。（`src/main.rs:1570-1613`）
- **点 `+` 后列表毫无反应**——草稿不再只停留在内存，而是立即作为置顶行显示。（`src/main.rs:566-583`）

### 测试

- 新增 6 个单测（保存时入库与草稿置顶）：`selecting_an_image_leaves_the_managed_dir_empty`、`save_ingests_pending_image_by_content_hash`、`save_fails_when_pending_source_disappeared`、`same_content_is_stored_once`、`draft_saves_to_top_while_import_appends`、`consecutive_drafts_stack_at_the_top`。（`src/main.rs:1913-2021`）
- 沿用 7 个拖拽落点纯函数单测。（`src/main.rs:1730-1881`）
- `cargo test` 全绿：核心库 11 + `src/main.rs` 13 + 验收 1 = **25 项通过**，零 warning。

### 文档

- 重写 `README.md`：按钮位置、`选择图片…` 已移除、`Ctrl+R` 语义改为内联编辑、保存时入库、版本号与长按 0.28 秒；并把**运行**前置条件（Microsoft Visual C++ 2015–2022 可再发行组件 x64，提供 `VCRUNTIME140.dll`）与对应疑难排查条目补齐，删去与「零外部运行时」矛盾的体积描述。

### 兼容性

- 数据库 schema（`SCHEMA_VERSION = 1`）与归档格式（`ARCHIVE_VERSION = 1`）**均未改变**，0.1.0 的数据目录与导出包可直接使用。
- 新增的 `Store::insert_at_top()` 只影响草稿落库路径，旧数据（`position` 从 0 连续排列）不受影响。

## [v0.1.0] - 2026-10-12

首个版本。

- 侧边栏示例列表：缩略图、短按选中、长按拖动重排
- 顶栏：选择图片、标题编辑、复制提示词、导出 / 导入图库、打开数据目录
- 五分区提示词编辑器：视觉上连成一个框，复制时剔除分区标题
- 存储：SQLite 元数据（`gallery.sqlite3`）+ 按 blake3 内容哈希命名的托管 `images/` 目录
- 导出 / 导入：`prompt-gallery-<时间戳>.zip`（JSON 元数据 + 图片），导入永不覆盖已有示例
- 开发工具：`seed` 冒烟数据生成器、`tests/acceptance.rs` 导出包结构契约测试
