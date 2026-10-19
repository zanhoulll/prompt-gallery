# AGENTS-RECIPES.md — 按症状查的踩坑配方

以**症状**为标题，多数按「症状 / 根因 / 做法」组织；遇到对应症状直接跳入。「定性看图」「实机测试骨架」「上下文被压缩」「版本流程」「文档维护」是参考节。

## 长按拖动无反应

**症状**：在某行上按住不动再拖动没有任何反应；或松手无反应。

**根因**：egui 的交互判定在按住超过 `max_click_duration`（默认 0.8s）后失效，且快照帧末才成立：

- `egui-0.36.2/src/context.rs:1488-1494` — `Flags::IS_POINTER_BUTTON_DOWN_ON` 只在快照阶段写入。
- `egui-0.36.2/src/interaction.rs:158-189` — `potential_click_id` / `potential_drag_id` 只在 `PointerEvent::Pressed` 那一帧设置。
- `egui-0.36.2/src/input_state/mod.rs:1488-1504` — `could_any_button_be_click()` 在 `time - press_start_time > max_click_duration` 时返回 false。

**做法**：

1. 控件只声明 `Sense::click()`，手势自己算。
2. 每帧从 `ctx.input` 取 `primary_pressed / primary_down / primary_released / press_origin / press_start_time / total_drag_delta / stable_dt`。
3. 状态机 `LongPress{Waiting, Armed, Cancelled}`：物理按下起算，280ms 达标 → Armed；达标前累计位移 > 6pt → Cancelled；达标后位移 > 4pt 才进入重排。
4. 手势存在期间每帧 `ctx.request_repaint()`（静止等待期也要，否则拖动虚影不会出现）。

逻辑写成纯函数才好单测：`long_press_transition` / `reorder_moved` / `gesture_outcome` / `autoscroll_speed` / `drag_insertion_index`。

## 截图整幅纯白 / 整幅纯黑（wgpu / DX 合成窗口）

**症状**：`PrintWindow` 抓到的图整幅同一个颜色 —— flag 1 是纯白，**flag 2 可能是纯黑**（本机实测 wgpu 窗口稳定得到 argb = -16777216）；界面明明在正常渲染。

**根因**：flag 1（PW_CLIENTONLY）对 wgpu / DX 合成窗口不适用。flag 2（PW_RENDERFULLCONTENT）也不是万能：它对 wgpu 窗口可能返回纯色。**纯色说明这张图无效，不说明窗口没渲染**。

**做法**：

1. 抓完先采样 9 点（`x = 2, bw/2, bw-3` × `y = 2, bh/2, bh-3`）：全同色 → 丢弃，改走 `Graphics.CopyFromScreen(ClientOrigin, ...)`。`tools/uicheck.ps1` 的 `Shot()` 已内置该探测与回退（tools/uicheck.ps1:207-254），会打印 `NOTE: PrintWindow uniform argb=… -> screen copy fallback`。
2. 屏拷贝受遮挡影响：**回退成功 ≠ 抓到了被测窗口**，先确认前台是它（见「合成输入无效 / 像素统计是别人的画面」）。
3. flag 2 渲染整窗（含标题栏）：先用 `GetWindowRect` 与 `ClientToScreen` 求客户区偏移 (dx, dy)，bitmap = 客户区尺寸 + 偏移，PrintWindow 后裁出客户区子矩形。

主窗口识别：枚举该 pid 的顶层窗口时**先按 `IsWindowVisible` 过滤**，再取客户区面积 > 300000 px 者。漏掉可见性过滤会选中本进程一个**不可见**的大窗口（实测 `__wglDummyWindowFodder`，客户区 1902×963），之后截图与坐标全部对到别处。句柄会变，每次重新解析。

## 绘制出的字符是空心方块（豆腐块 / 缺字形）

**症状**：界面上出现规整的空心方框或问号，位置固定、大小一致。

**根因**：该**码位在字体栈里全部缺字形**，不是布局多画了一个 rect。egui 0.36 的 Proportional 族顺序 = Ubuntu-Light → NotoEmoji-Regular（scale 0.81）→ emoji-icon-font（scale 0.90），末尾由 `main.rs::install_cjk_fonts` append CJK（msyh / simhei / simsun）。前三者不含汉字、MSYH 不含 emoji，两个方向的缺字形都会出现。

本机实测（用 `tools/cmapcheck.py` 解析 ttf/ttc 的 cmap，逐码位判定）：

- U+270E（✎）在 Ubuntu-Light / NotoEmoji / emoji-icon-font / msyh 中**全缺** —— 侧栏悬停铅笔就是它；
- U+1F512（🔒）/ U+1F513（🔓）在 NotoEmoji 中都有；
- U+1F58A（🖊）只有 emoji-icon-font 有，NotoEmoji 没有。

**做法**：

1. 排查顺序：先查缺字形，再查布局或命中区。
2. 用 `python tools/cmapcheck.py -cp 270E,1F512,1F513 <字体.ttf|ttc>` 列出目标码位在每个候选字体里是否存在（脚本思路：ttcf/ttf 表目录 → cmap format 4/12）。
3. **成对图标要分别核对**，别假设对称 —— 🔒 与 🖊 的分布正好相反。

## 按布局图的几何去量截图，整体差 1.25 倍

**症状**：拿 `docs/ui-layout-map.md` 的数去量截图，整体差 1.25 倍，或出现固定漂移。

**根因**：布局图记的是 **pt（客户区原点，`pixels_per_point = 1.25`）**；截图、`GetWindowRect`、`ClientToScreen`、`CopyFromScreen` 全是**物理像素**。客户区 `with_inner_size([1180, 760])` pt = 1475×950 px。

**做法**：

- 像素 ÷ 1.25 = pt（反向量测时用它换算）。
- `PrintWindow(h, hdc, 2)` 渲染整窗后裁客户区时，**裁剪偏移 (dx, dy) 也用物理像素**；与 pt 混用会整体漂移。
- 布局图**不再存精确坐标**（每改一次就过期）：它只给规则，数字用 `hookdrive` 出图 + `imgdiff.py` 现场量。
- 只重测改动影响到的区域，未改区域沿用上次实测值（见布局图「复测 → 增量重测」）。

## 自写解析脚本崩溃在非目标文件上 / 参数写死、缺用法说明

**症状**：`struct.error: unpack requires a buffer of 16 bytes`（或类似的魔数 / 长度错误）。

**根因**：用 glob 抓目录时把附带文件一并喂进脚本 —— 例如 `**/epaint_default_fonts-0.36.2/fonts/*` 会带上 `OFL.txt`、`Hack-Regular.txt`。

**做法**：调用方按扩展名过滤（`*.ttf` / `*.ttc`），脚本内再校验魔数（`ttcf` / `0x00010000`）兜底。工具自己不会筛。

同类问题：这类脚本通常是**一次性**的，用法注释和参数处理都不完整 —— 本次那个 cmap 脚本把待探测的码位写死在源码里，读了前 55 行很容易误以为它接受任意码位（已据此重写为 `tools/cmapcheck.py`：码位改从 `-cp` 传入、非字体文件直接跳过、用法写在文件头）。**调用前先扫一眼入口与参数处理**（`sys.argv` / `param()`），别读一半就当通用工具使。

## 定性看图，定量量测（视觉断言怎么选）

**事实**：截图能以图像附件进入对话，可逐字读出界面文字与颜色（实测 1475×950 整窗可读）。

**做法**：**定性看图、定量量测**。看图有三条真实限制：

- **精度**：能看出「有个方块」「大致居中」，看不出「8.0pt 还是 8.3pt」。坐标 / 像素差断言必须用 `tools/imgdiff.py`（diff / color / scan / crop），不靠眼睛。
- **折叠**：上下文压缩会把图像像素抽走，只留一行 `[folded images: …]`，要 `decompress({ imageRef })` 才能恢复。**看图得到的结论必须当场写成文字**，别指望压缩后还能回看。
- **成本**：一张 1475×950 截图约 150KB 图像数据加视觉 token，连看十几张很贵；`imgdiff.py` 的数字结论只要几十字节。

## 合成输入无效 / 像素统计是别人的画面

**症状**：合成输入后应用毫无反应（连状态栏文案都不变）；或截图里的内容与像素统计是别的应用（浏览器、全屏游戏）的画面。

**根因**：

- 窗口不在前台时，Windows 把**第一次**按下用于激活窗口、不交给应用 → 整段手势无声；`SetForegroundWindow` 单独调用会失败，`PostMessage` 直投 `WM_MOUSEMOVE` / `WM_LBUTTONDOWN` / `WM_LBUTTONUP` 对 egui 无效。
- 抢到前台也只是一次性的：激活会吞掉第一下点击，别的应用（游戏、浏览器）随时能抢回去。
- 有其它窗口盖住被测应用时，屏拷贝抓到的是别人的画面，像素统计全是噪声。

**首选做法**：用**应用内测试钩子**（`tools/hookdrive.py` + ADR-0008）——注入的是 egui 真实输入、截图走 GPU 回读、每步有应答，因此不吃前台、也不受遮挡影响（窗口最小化时没有帧）。下面这套 Alt 键技巧只在没有钩子、必须走真实 OS 输入时用。

**做法**（Alt 键技巧，**每一步输入 / 截图前重做**）：

1. `keybd_event(VK_MENU=0x12, 0, 0, 0)` → `keybd_event(0x12, 0, KEYEVENTF_KEYUP=2, 0)` → `SetForegroundWindow(hwnd)` → `BringWindowToTop(hwnd)`；用 `GetForegroundWindow()` 核对确实抢到了。
2. 把「重抢」放进**每个** `down` / `up` / `move` / `shot` 之前，不要只在启动时抢一次（`tools/uicheck.ps1` 只在启动阶段抢前台；逐步驱动的脚本要自己写个 `regrab()` 在每步前调）。
3. 输入是否落地**以读库为准**（`python tools/dbdump.py --dir <数据目录>`），不看像素、也不看「界面没反应」：本机一次「锁图标点击无效」最终查出是 Chrome / 游戏占前台，产品代码没问题。
4. 抢不到、或截图确认抓到别人的画面 → 这一轮作废，别据此得出「功能没生效」。
5. 打字用 `SendInput` + `KEYBDINPUT(KEYEVENTF_UNICODE)`（flags 4=按下 / 6=抬起）。

## 钩子场景收尾卡住（`quit` 被「未保存草稿」拦下）

**症状**：`python tools/hookdrive.py` 报「quit 后进程未退出」/ `expect-exit` 超时，但界面每一步都正常响应。

**根因**：带未保存草稿时关窗会走「草稿尚未保存」确认弹窗（`src/main.rs` 的 `close_confirm`，ADR-0006 D9），`quit` 发的是正常关窗，被这个模态拦下；驱动不会替你点它。

**做法**：场景收尾一律用 `kill`（强制结束，不算失败）。只有确认当前没有脏状态时才用 `quit` + `expect-exit`；要测关窗保护本身，就把它当被测行为，显式点弹窗按钮。

## 文本框看着点进去了却打不进字（命中区只有 `desired_rows` 高）

**症状**：`click x y` 落在多行文本框的可见边框内，随后 `text ...` 一个字都没进去；换个更靠上的点就正常。

**根因**：文本框的**交互矩形**由 `TextEdit::desired_rows(n)` 决定（本项目设 6 行，约 6 行高），其余面积是外层 `ScrollArea` 的背景——看着在框里，其实已经越过控件下边界。

**做法**：点在文本第一行到第 6 行之间。本机 1180×760 窗口、框顶内缩 30pt 时，可点区约物理 `y 445..565`（用 `(600,500)` / `(600,550)`）；`(600,600)` 已在框内但点不动。要更大的可点区就 `ui.add_sized([w, h], TextEdit…)`，别只调 `desired_rows`。

## 按钮「看着点中了」却没反应（坐标是目测的）

**症状**：按截图目测的坐标点按钮，界面毫无反应；同一个按钮换一个点就生效。

**根因**：目测坐标有 ±10 px 级误差，而按钮的可见底色、边框、命中矩形三层边界未必重合，落在边缘外时 egui 直接当点在空白处。

**做法**：**别连试第二次，先量**。用 Pillow 扫特征色求 bbox 取几何中心：确认框危险按钮有强调色边框，扫 `R>140 且 R-G>45 且 R-B>45` 得 bbox `x 883..949 / y 518..554` → 中心 `(916,536)`；此前目测的 `(905,541)` 落在框内却不生效。`tools/imgdiff.py` 的 `scan` 可做同类统计。偏差记录：同一次会话里「侧栏首行」的命中矩形是物理 `y 145..210`，点在 `y=137` 会落到行上方空白。

## 「置灰 / 变暗」在浅色主题下反而更亮（`gamma_multiply` 连 alpha 一起乘）

**症状**：禁用态文字在深色主题下确实更暗，切到浅色主题反而比常态更亮、更显眼。

**根因**：`Color32` 是预乘色，`gamma_multiply(k)` 把 alpha 一起乘（`ecolor-0.36.2/src/color32.rs:269-281`）→ 真实效果是「**向背景靠拢**」：深底靠拢变暗、浅底靠拢变亮。

**做法**：语义写成「**降低对比**」，不写「更暗」。实测三态取区域内最亮/最暗像素：深色主题 96 / 150 / 220（空 / 常态 / 悬停），浅色主题 164 / 101 / 31 —— 亮度顺序相反，「对比递增」一致。要恒定变暗就只改 RGB、别动 alpha。

## 单测期望值用同一批常量算（常量写错也测不出）

**症状**：布局 / 几何单测全绿，但把实现里的常量改错，测试照样全绿。

**根因**：期望值是 `avail.w * IMAGE_SHARE` 这类由被测常量算出来的表达式——测试与实现同源，错一起错。

**做法**：期望值写**字面量**（`495.6` / `467.48` / `727.88`），注释写明「常量改错必须让这条失败」；只有安全网类断言（如退化阈值）可以引用常量。

## 构建报「拒绝访问」（os error 5）

**症状**：`failed to remove file ...prompt-gallery.exe: 拒绝访问。`

**根因**：上一轮的 exe 还在运行。

**做法**：`Get-Process prompt-gallery | Stop-Process -Force` 后重建；测试脚本的 finally 里必须 `CloseMainWindow` + `Kill`。

## 内嵌脚本报 Expected unicode escape / 中文乱码

**症状**：TS 模板字面量解析失败；或 Python 报告 `UnicodeEncodeError: 'gbk' codec can't encode character`。

**做法**：

- TS 模板字面量里的字面反引号会终止模板 → 脚本内用 `chr(96)` 拼接（或占位符替换）。
- 路径写正斜杠（TS 会静默吞掉 `\U` 之类无效转义）。
- Python 写完文件后 `print` 含 ⚠️ 的报告会抛 `UnicodeEncodeError` —— **文件其实已经写成**，只是报告失败；用 read 工具复核，或 `sys.stdout.reconfigure(encoding='utf-8')`。

## 批量改文档出现错行 / 静默 no-op

**症状**：逐行改完后行错位，或改动静默没生效。

**根因**：按下标逐行 `setline` / `insert` 极易错位，且不报错。

**做法**：整块替换 `L[start:end+1] = block`；改完用 read / grep 复核关键行，不信脚本自己的 print 报告。

## 加了表结构却报 `no such column`（迁移静默失效）

**症状**：`CREATE TABLE` 里加了列、代码里也 `SELECT` 了它，运行时才报 `no such column: <列名>`；**新装用户正常、老库才炸**。

**根因**：本仓库的迁移是**提前返回**式（`src/store.rs:316`）：

```rust
let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
if version >= SCHEMA_VERSION { return Ok(()); }
```

于是「只往建表语句里加列、不升 `SCHEMA_VERSION`」时，老库的 `version(=1) >= SCHEMA_VERSION(=1)` 成立 → 直接 return → `ALTER TABLE` 永远不会执行。走建表分支的新装用户反而是好的，只有已存在的数据库会炸 —— 是最容易漏测的那条路径。

**做法**：

1. **加列 = 升 `SCHEMA_VERSION`**，迁移写三分支：`version == 0` 建表（带新列）并写版本号；`version < N` 执行 `ALTER TABLE … ADD COLUMN …` 再写版本号；`version >= N` 才 return。
2. 数据库格式版本用 `PRAGMA user_version`（`src/store.rs:20`），与 `ARCHIVE_VERSION`（导出包格式版本，`src/archive.rs:23`）各管一段：只有数据库结构变了才升前者，`archive_version` 可以保持不变。
3. 升级路径必须留一条**老库实测**：把 `%APPDATA%\PromptGallery\` 那份旧库复制一份再跑，不只测新装。

## 旧库升级只测了「有内容」，空库把用户挡在门外

**症状**：升级后一打开就弹「数据库 schema 是 v2，不提供迁移」，用户以为程序坏了；他的旧库里其实**一条示例都没有**。

**根因**：迁移的「拒绝」分支只造了「有内容的旧库」夹具，漏了「旧库文件存在、但两张表都空」——装过旧版却没存东西的人 100% 命中这条，是最常见的现实分布。

**做法**：迁移夹具穷举四态：`version == 0` 新库 / 旧版本**空**库 / 旧版本有内容 / 比当前更高版本。空库可以整表丢弃后按新 schema 重建（`legacy_library_is_empty()`），只有有内容的旧库才拒绝并给改名备份指引。

## 小图标 / 叠层控件点不动（命中区被盖）

**症状**：行右端的小图标点了没反应，同一行的整行点击却正常。

**根因**：egui 命中判定里同层重叠**不丢弃**被完全覆盖的控件 —— `egui-0.36.2/src/hit_test.rs:141-156` 只在 `layer_id` 不同时才把被包含者判为 hidden；同距离取**后注册**者（`hit_test.rs:412-438` 的 `find_closest_within` 用 `<=` 替换），所以「行先 `allocate_exact_size` 注册、图标后 `ui.interact` 注册」时图标本应胜出。

**做法**：先用这条确认命中归属；若归属没问题，就别再往 UI 里找 —— 用读库 / 日志确认输入到底有没有到达（见「合成输入无效 / 像素统计是别人的画面」）。本机一次「锁图标点不动」最后查出是环境问题。

## 实机测试骨架（tools/）

`tools/uicheck.ps1` 的头部注释即用法：`-Preset drag|click|fixed|scrollup|map|lock`，步骤 DSL `shot:名 / down:x:y / move:x:y / up / sleep:毫秒 / wheel:格数`。
只记从文件里看不出来的事实：

- 坐标是**客户区像素**，隐含 125% 缩放（1.25 px/pt）。
- 需要交互式桌面，不能进 CI。
- 每次运行重建 data 目录（`-KeepData` 保留）。
- `-Scene` 传数组要用直接调用 `& 'tools/uicheck.ps1' -Scene @('…')`，`pwsh -File` 会把元素错绑到 `-Seed`。
- 它只在**启动阶段**抢前台；需要「每步重抢 + 读库校验」的场景写一个独立驱动脚本，别改坏它（拍摄已内置纯色回退，见「截图整幅纯白 / 整幅纯黑」）。
- 首选 `tools/hookdrive.py`（应用内测试钩子，见 ADR-0008）：`--data <目录> --scene <场景文件> [--seed N] [--exe <exe>] [--timeout 秒] [--keep]`；场景 DSL `move/down/up/click/text/key/shot/wait/quit` + 驱动侧 `close/kill/minimize/expect-exit`；只跑 debug 产物（release 里钩子是空 stub）。
- 辅助：`tools/imgdiff.py`（diff / color / scan / crop）、`tools/dbdump.py`（表 / 行 / 托管图片数 / locked；用 `--dir <数据目录>`，传目录位置参数会被当成库文件）、看分区文本要另写小脚本（dbdump 只打印 examples）。

## 文档维护

- 代码落点只写**函数名**锚点：行号每次改动都漂移，写进文档就变成假坐标（UI 布局图 v0.1.4 起已不再存行号）。
- 临时任务书 / 临时产物放 `target/`，不放 `docs/`——`docs/` 是 `docs/directory-tree.md` 常驻清单的来源，放入用完即删的文件会让清单失真。

## 上下文被压缩（担心丢信息 / 重复劳动）

会话由 `billion-context` 自动压缩。三种失效形态都出现过：显著性丢失 → 重复劳动；错误结论被固化 → 返工；精确数字被抹平 → 重测。**做法**：按 AGENTS.md「落盘优先」执行；账本与可折叠区间用 `acp_cache` / `acp_status` 查。

## 版本流程为什么这么切（实现 → 独立验证 → 落账）

- **实现会话 = 第一次 stranger test**：交接文档不够清楚时，实现第一步就会卡住——这是最早能暴露文档缺陷的位置；会话边界绑在「文档」上，不为省 token 而合并。
- **验证用子代理、不用新会话**：子代理不占主会话上下文，又能恢复「陌生人读文档」的检验；只给文档 / 代码 / 产物，不给实现者的叙述，否则验证退化成自我复述。
- **规格 bug 先改文档**：文档与代码分叉后，下一次会话会继承一个谎言。
- **完成 = 独立验证通过**：上一版规划任务书的「## 进度」全停在 `⏳ 待实现`，而版本早已发布——第二个真相源一定会过期，完成清单只认 `CHANGELOG.md`。
- **验收项要贴用户原话**：一次任务书把「横图下方左右分栏」转述成「上下两段」，代码、文档、验收标准三份同源出错，两轮验证都没抓到，最后靠回看原始需求才发现。转述一旦出错，实现与文档互相验证等于自证；凡涉及布局 / 文案 / 数值取舍的验收项，把用户原话整句抄进去。
