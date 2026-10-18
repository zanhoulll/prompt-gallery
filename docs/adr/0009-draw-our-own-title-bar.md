# 自绘标题栏：`with_decorations(false)` + 透明窗口，接受丢失系统标题栏能力

v0.1.4 的视觉重做要求窗口顶部与深/浅色主题一致：系统标题栏由 DWM 绘制（深色下仍是系统灰、无法跟随应用色板），应用图标也要作为品牌元素出现在标题栏里。因此主窗口改为客户区自绘：`ViewportBuilder::with_decorations(false).with_transparent(true)`，`clear_color` 返回全透明，根 `Frame` 用 `Palette::canvas` 填充、圆角 10（最大化时 0）、1pt `border` 描边、内边距 1pt（这 1pt 就是自绘边框的位置）；标题栏是 `egui::Panel::top("title_bar").exact_size(40.0)`，填充 `surface_raised`，左侧放 18×18 的程序化图标（`src/icon.rs`）、应用名与「数据管理 / 关于 / 外观」三个 chrome 按钮，右侧放最小化 / 最大化还原 / 关闭三个 46×40 窗口按钮。

自绘意味着这些系统行为要自己补：整条标题栏 `ui.interact(..., Sense::click_and_drag())`，`drag_started_by(Primary)` 且指针不在任何子控件上时发 `ViewportCommand::StartDrag`（子控件先注册 `Sense::click()`，命中即跳过拖动，否则按钮点不动）；双击发 `ViewportCommand::Maximized(!maximized)`；窗口按钮分别发 `Minimized(true)`、`Maximized(!maximized)`、`Close`（走应用自己的草稿拦截路径）。八向缩放用 8 个 `Order::Foreground` 的无形 `Area` 热区（边 5pt、角 12pt）覆盖在面板之上，`drag_started` 时发 `ViewportCommand::BeginResize(ResizeDirection::*)`，并给每个热区设对应的 `Resize*` 光标；最大化时跳过。热区用 `ctx.content_rect()`（局部坐标，egui 0.36 没有 `screen_rect`）定位，用屏幕坐标会把热区整体推出窗口。

代价与边界（明确接受）：`with_decorations(false)` 之后 Windows 的 **Snap Layouts 悬停面板**不再出现（它只挂在系统最大化按钮上）、**Alt+Space 系统菜单**消失、**DWM 窗口阴影**不再绘制；透明窗口在部分显卡驱动 / 远程桌面下可能闪烁或边缘发黑。补偿是自绘圆角 + 1pt 描边 + 弹窗阴影（`Palette::shadow`），观感接近但不等于系统阴影；Snap Layouts 可用 Win+Z 或拖到屏幕边缘的 Aero Snap 代替。备选方案：保留系统装饰（视觉不统一，直接否）、引入 `window-vibrancy` / `decorum` 等第三方 crate 做客户区装饰（项目硬规则不新增依赖，且它们同样不恢复 Snap Layouts 悬停面板），因此不做。

窗口图标不是资源文件：`src/icon_render.rs` 的 `render(size)` 在 256 设计稿坐标里画圆角蓝底 + 白相框 + 天空/太阳/双层山，按 4×（≥48px）或 8×（<48px）超采样后盒式降采样，圆角外 alpha = 0；`main.rs` 的 `app_icon()` 只把 `render(64)` 包成 `egui::IconData`，标题栏用 `paint()` 缓存纹理。任务栏按钮与资源管理器并不看这个窗口级图标（v0.1.4 实测：`WM_GETICON` 已返回我们的图标，Win11 任务栏仍显示系统默认图标），它们取 exe 内嵌的 `.rsrc` 资源——同一份像素由 `build.rs` 在链接期编进 exe，见 ADR-0010；因此仍然没有需要随包分发的图片文件。
