//! Prompt Gallery —— 本地提示词图库。
//!
//! 需求与决策来源：
//! * `CONTEXT.md`：术语表；窗口标题、导出文件名前缀、五个分区标题等用户可见文案。
//! * `docs/adr/0001-egui-eframe-for-low-memory-ui.md`：egui/eframe，隐藏滚动条、
//!   长按拖拽重排、不可选中灰字都要自己画。
//! * `docs/adr/0002-prompt-editor-visual-continuity-not-single-document.md`：提示词编辑器
//!   由 5 个 TextEdit 在同一滚动容器里无缝拼接，视觉上是一个框。
//! * `docs/adr/0003-sqlite-metadata-plus-hash-named-image-files.md`：SQLite + 内容哈希图片文件。
//!
//! 依赖版本注记：本项目使用 egui/eframe 0.36。该版本的 `eframe::App` 用 `ui(&mut Ui, &mut Frame)`
//! 而不是 `update(&Context, …)`，并且面板统一为 `egui::Panel::{left,right,top,bottom}`
//! （`show(ui, …)` 接收 `&mut Ui`）。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use egui::{
    Align, Color32, CornerRadius, Frame, Id, Key, Layout, Modifiers, Rect, RichText, ScrollArea,
    Sense, Stroke, StrokeKind, TextEdit, Vec2,
};

use prompt_gallery::archive;
use prompt_gallery::images::{fit_size, ImageCache};
use prompt_gallery::model::{
    now_unix, Example, Sections, SectionIndex, SECTION_COUNT, SECTION_TITLES,
};
use prompt_gallery::store::{self, Store};

/// 窗口标题；同时是应用显示名（CONTEXT.md）。
const APP_TITLE: &str = "Prompt Gallery";
/// 侧边栏默认宽度。
const SIDEBAR_WIDTH: f32 = 248.0;
/// 分区标题的灰色（用 `Label::selectable(false)` 保证不可选中）。
const HEADER_GRAY: Color32 = Color32::from_gray(150);
/// 分区之间的细分隔线。
const SEAM_COLOR: Color32 = Color32::from_gray(72);
/// 图片框背景。
const IMAGE_BOX_FILL: Color32 = Color32::from_rgb(28, 30, 34);
/// 「长按拖拽重排」的判定时间（§3.2：280ms，比旧值更迟钝，避免误触重排）。
const REORDER_HOLD: Duration = Duration::from_millis(280);
/// 拖拽产生位移的阈值，用于区分点击与重排。
const REORDER_SLOP: f32 = 4.0;
/// 状态文案自动淡出的秒数。
const STATUS_TTL: f64 = 12.0;

/// 支持的图片后缀（对话框过滤用）。
const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_TITLE)
            .with_app_id("prompt-gallery")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([880.0, 560.0])
            .with_icon(app_icon()),
        // wgpu 在 Windows 上比 OpenGL 更稳（无需系统 GL 驱动）。
        renderer: eframe::Renderer::Wgpu,
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        APP_TITLE,
        options,
        Box::new(|cc| {
            // 中文界面必须挂上系统 CJK 字体，否则全是豆腐块。
            install_cjk_fonts(&cc.egui_ctx);
            match Store::open() {
                Ok(store) => Ok(Box::new(GalleryView::new(store))),
                Err(err) => Ok(Box::new(FatalView {
                    message: format!("{err:#}"),
                })),
            }
        }),
    )
}

/// 打开数据库失败时的兜底界面：把原因显示出来，而不是静默退出。
struct FatalView {
    message: String,
}

impl eframe::App for FatalView {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            ui.heading(format!("{APP_TITLE} 无法启动"));
            ui.add_space(8.0);
            ui.label("数据库无法打开，请检查数据目录的读写权限后重试：");
            ui.add_space(4.0);
            ui.monospace(self.message.clone());
            ui.add_space(12.0);
            if let Ok(dir) = store::resolve_data_dir() {
                ui.label(format!("数据目录：{}", dir.display()));
            }
        });
    }
}

// ---------------------------------------------------------------- 应用状态

struct GalleryView {
    store: Store,
    /// 当前示例（当前示例 / 草稿）的工作副本。
    current: Option<Example>,
    /// 数据库里的那一份；与 `current` 不同即「有未保存改动」。
    baseline: Option<Example>,
    /// 数据库文件所在目录（缓存成 String，便于 UI 显示与导出）。
    data_dir: String,
    cache: ImageCache,
    /// 待入库图片（Pending Image）：只记源路径，保存时才按内容哈希写入托管目录（ADR-0004）。
    pending_image: Option<PendingImage>,
    status: String,
    status_time: f64,
    /// 侧边栏标题的内联编辑状态（§2）。
    editing_title: Option<InlineEdit>,
    /// 关于弹窗（§5）。
    about_open: bool,
    /// 删除确认弹窗。
    delete_confirm: bool,
    /// 切换示例前的未保存改动确认；目标 id（`None` 表示新建草稿）。
    pending_select: Option<Option<i64>>,
    /// 导出弹窗与路径缓冲区。
    export_open: bool,
    export_path: String,
    /// 侧边栏拖拽重排状态。
    drag_row_id: Option<i64>,
    drag_moved: bool,
    drag_started: Instant,
    drag_target: Option<usize>,
}

impl GalleryView {
    fn new(store: Store) -> Self {
        let data_dir = store.data_dir().to_string_lossy().to_string();
        let mut view = Self {
            store,
            current: None,
            baseline: None,
            data_dir,
            cache: ImageCache::new(),
            pending_image: None,
            status: String::new(),
            status_time: f64::NEG_INFINITY,
            editing_title: None,
            about_open: false,
            delete_confirm: false,
            pending_select: None,
            export_open: false,
            export_path: String::new(),
            drag_row_id: None,
            drag_moved: false,
            drag_started: Instant::now(),
            drag_target: None,
        };
        // 启动后自动选中第一条；空图库则直接给一个草稿，省掉一次点击。
        match view.store.rows().first().map(|row| row.id) {
            Some(id) => view.load_example(id),
            None => view.create_draft(),
        }
        view
    }

    fn current_id(&self) -> Option<i64> {
        self.current.as_ref().and_then(|example| example.id)
    }

    /// 是否有未保存改动。
    ///
    /// 空草稿（标题、分区都为空且没有待入库图片）不算改动，切走时不弹确认框。
    /// 待入库图片也要算进来：ADR-0004 后选图只记源路径，不写进当前示例。
    fn dirty(&self) -> bool {
        let Some(current) = self.current.as_ref() else {
            return false;
        };
        if current.is_blank() && self.pending_image.is_none() {
            return false;
        }
        match &self.baseline {
            Some(baseline) => current.differs_from(baseline) || self.pending_image.is_some(),
            None => true,
        }
    }

    fn load_example(&mut self, id: i64) {
        match self.store.load(id) {
            Ok(Some(example)) => {
                self.baseline = Some(example.clone());
                self.current = Some(example);
                self.pending_image = None;
                self.editing_title = None;
                self.cache.drop_preview();
                self.status = String::new();
            }
            Ok(None) => {
                self.status = "该示例已不存在".to_string();
                self.status_time = 0.0;
            }
            Err(err) => self.set_error(err),
        }
    }

    fn create_draft(&mut self) {
        self.baseline = None;
        self.current = Some(Example::draft(self.store.next_position()));
        self.pending_image = None;
        self.editing_title = None;
        self.cache.drop_preview();
    }

    /// 切换当前示例；若有未保存改动则先弹确认。
    fn select(&mut self, id: Option<i64>) {
        if id == self.current_id() && self.baseline.is_some() {
            return;
        }
        if self.dirty() {
            self.pending_select = Some(id);
            return;
        }
        match id {
            Some(id) => self.load_example(id),
            None => self.create_draft(),
        }
    }

    fn perform_pending_select(&mut self) {
        let target = self.pending_select.take().flatten();
        match target {
            Some(id) => self.load_example(id),
            None => self.create_draft(),
        }
    }

    fn set_status(&mut self, ctx: &egui::Context, message: String) {
        self.status = message;
        self.status_time = ctx.input(|i| i.time).max(0.0);
    }

    fn set_error(&mut self, err: anyhow::Error) {
        self.status = format!("错误：{err:#}");
        // 错误消息不自动淡出。
        self.status_time = 0.0;
    }

    // ------------------------------------------------------------ 底部动作

    /// 保存：草稿落库到列表顶部；此时才把待入库图片按内容哈希写入 images/（ADR-0004）。
    fn save_current(&mut self) {
        // 保存时才读源文件并入库；源文件缺失则报错，不静默丢图。
        let ingested = match self.pending_image.as_ref() {
            Some(pending) => match ingest_pending_image(&self.store, pending) {
                Ok(pair) => Some(pair),
                Err(err) => {
                    self.set_error(err);
                    return;
                }
            },
            None => None,
        };
        if let Some((hash, ext)) = ingested {
            if let Some(example) = self.current.as_mut() {
                example.image_hash = Some(hash);
                example.image_ext = Some(ext);
            }
        }

        let store = &mut self.store;
        let mut result: anyhow::Result<()> = Ok(());
        if let Some(example) = self.current.as_mut() {
            example.title = example.title.trim().to_string();
            result = match example.id {
                Some(_) => store.update(example),
                // 草稿落库到顶部：position 0，其余示例依次后移（§8）。
                None => store.insert_at_top(example).map(|_| ()),
            };
        }
        match result {
            Ok(()) => {
                self.baseline = self.current.clone();
                self.pending_image = None;
                self.editing_title = None;
                self.status = "已保存".to_string();
                self.status_time = f64::NAN; // 下一帧用 ctx 时间刷新
            }
            Err(err) => self.set_error(err),
        }
    }

    /// 另存为：当前示例成为新草稿，下次保存时落库为新示例（原示例不变）。
    fn save_as(&mut self) {
        let position = self.store.next_position();
        let Some(current) = self.current.as_mut() else {
            return;
        };
        current.id = None;
        current.position = position;
        current.created_at = now_unix();
        current.updated_at = current.created_at;
        self.baseline = None;
        self.editing_title = None;
        self.status = "已另存为副本，点「保存」写入图库".to_string();
        self.status_time = f64::NAN;
    }

    fn delete_current(&mut self) {
        let Some(id) = self.current_id() else {
            return;
        };
        // 删除后优先选中它的前一条，其次后一条；都没有就进草稿。
        let rows = self.store.rows().to_vec();
        let index = rows.iter().position(|row| row.id == id);
        let next = index.and_then(|index| {
            if index > 0 {
                Some(rows[index - 1].id)
            } else {
                rows.get(1).map(|row| row.id)
            }
        });
        match self.store.delete(id) {
            Ok(()) => {
                self.baseline = None;
                self.current = None;
                self.pending_image = None;
                self.editing_title = None;
                self.cache.drop_preview();
                match next {
                    Some(next_id) => self.load_example(next_id),
                    None => self.create_draft(),
                }
                self.status = "已删除".to_string();
                self.status_time = f64::NAN;
            }
            Err(err) => self.set_error(err),
        }
    }

    /// 选图/拖图：只记住源文件路径，不再当场写入托管目录（ADR-0004）。
    ///
    /// 这里只读一次源文件做格式/大小校验与尺寸提示；真正的入库发生在保存时。
    fn set_pending_image(&mut self, ctx: &egui::Context, path: &Path) {
        let size = match std::fs::read(path) {
            Ok(bytes) => {
                if store::ext_for_bytes(&bytes).is_none() {
                    self.set_status(ctx, format!("无法识别的图片格式：{}", path.display()));
                    return;
                }
                if bytes.len() as u64 > MAX_IMAGE_BYTES {
                    self.set_status(
                        ctx,
                        format!("图片过大（{} MB），已拒绝载入", bytes.len() / 1024 / 1024),
                    );
                    return;
                }
                image_size_of(&bytes)
            }
            Err(err) => {
                let err = anyhow::Error::new(err)
                    .context(format!("读取图片失败：{}", path.display()));
                self.set_error(err);
                return;
            }
        };
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let hint = match size {
            Some((w, h)) => format!("已选择图片 {w}×{h}（保存后入库）"),
            None => "已选择图片（保存后入库）".to_string(),
        };
        self.pending_image = Some(PendingImage {
            path: path.to_path_buf(),
            label,
            key: format!("pending:{}", path.display()),
        });
        self.cache.drop_preview();
        self.set_status(ctx, hint);
    }

    fn export_to(&mut self, ctx: &egui::Context) {
        let mut target = PathBuf::from(self.export_path.trim());
        if target.as_os_str().is_empty() {
            self.set_error(anyhow::anyhow!("导出路径为空"));
            return;
        }
        if target.extension().is_none() {
            target.set_extension("zip");
        }
        match archive::export(&self.store, &target) {
            Ok(count) => {
                self.export_open = false;
                self.set_status(ctx, format!("已导出 {count} 条 → {}", target.display()));
            }
            Err(err) => self.set_error(err),
        }
    }
}

/// 单张图片的字节上限，防止误拖入超大文件把内存打满。
const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;

/// 只读图片头取尺寸；失败返回 None（不阻塞保存流程）。
fn image_size_of(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// 待入库图片到托管文件：读取源路径，按内容哈希写入 images/，返回 (hash, ext)。
///
/// 边界（ADR-0004）：源文件在保存前被移动/删除则返回错误，让保存失败并提示重新选择，
/// 而不是静默丢图；内容哈希命名让重复图片复用同一个文件（另存为时不重复写）。
fn ingest_pending_image(store: &Store, pending: &PendingImage) -> anyhow::Result<(String, String)> {
    let bytes = std::fs::read(&pending.path).map_err(|err| {
        anyhow::Error::new(err).context(format!(
            "读取待入库图片失败，请重新选择：{}",
            pending.path.display()
        ))
    })?;
    let ext = store::ext_for_bytes(&bytes).ok_or_else(|| {
        anyhow::anyhow!("无法识别的图片格式，请重新选择：{}", pending.path.display())
    })?;
    if bytes.len() as u64 > MAX_IMAGE_BYTES {
        anyhow::bail!("图片过大（{} MB），已拒绝载入", bytes.len() / 1024 / 1024);
    }
    store.store_image_bytes(&bytes, ext)
}

// ---------------------------------------------------------------- 界面

impl eframe::App for GalleryView {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // 拖进窗口的图片文件（与侧边栏重排不冲突：那是内部拖拽，这里是外部文件）。
        // ADR-0004：只记住源路径，保存时才按内容哈希入库。
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(file) = dropped.into_iter().next() {
            let path = file.path().to_path_buf();
            self.set_pending_image(&ctx, &path);
        }

        self.handle_shortcuts(&ctx);
        self.sidebar(ui);
        self.bottom_bar(ui);
        self.central(ui);
        self.dialogs(&ctx);
        self.tick_status(&ctx);
        self.window_title(&ctx);

        if self.editing_title.is_some()
            || self.delete_confirm
            || self.pending_select.is_some()
            || self.about_open
        {
            ctx.request_repaint();
        }
    }
}

impl GalleryView {
    /// 状态时间戳的延迟刷新：`set_status` 拿不到 ctx 时先写 NaN，这里补上。
    fn tick_status(&mut self, ctx: &egui::Context) {
        if self.status_time.is_nan() {
            self.status_time = ctx.input(|i| i.time);
        }
    }

    /// 窗口标题显示当前示例，便于任务栏识别；有改动时加 `*`。
    fn window_title(&self, ctx: &egui::Context) {
        let title = match &self.current {
            Some(example) => {
                let mark = if self.dirty() { "*" } else { "" };
                format!("{mark}{} — {APP_TITLE}", example.display_title())
            }
            None => APP_TITLE.to_string(),
        };
        if ctx.input(|i| i.viewport().title.as_deref() != Some(title.as_str())) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (save, save_as, rename) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::CTRL, Key::S),
                i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::S),
                i.consume_key(Modifiers::CTRL, Key::R),
            )
        });
        if save_as {
            self.save_as();
        } else if save {
            self.save_current();
        }
        if rename {
            // §2：Ctrl+R 改为当前行的侧边栏内联编辑，不再弹重命名窗。
            let target = self.current_id();
            self.begin_inline_edit(target);
        }
    }

    fn import_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Prompt Gallery 导出包", &["zip"])
            .pick_file()
        else {
            return;
        };
        match archive::import(&mut self.store, &path, archive::ImportMode::Append) {
            Ok(report) => {
                let mut message = format!("已导入 {} 条示例", report.added);
                if report.remapped_ids > 0 {
                    message.push_str(&format!("，其中 {} 条另分配了 id", report.remapped_ids));
                }
                if report.missing_images > 0 {
                    message.push_str(&format!("，{} 张图片在包中缺失", report.missing_images));
                }
                if report.skipped > 0 {
                    message.push_str(&format!("，跳过 {} 条空示例", report.skipped));
                }
                self.set_status(ctx, message);
            }
            Err(err) => self.set_error(err),
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(SIDEBAR_WIDTH)
            .size_range(180.0..=420.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("示例 {}", self.store.row_count()))
                            .color(HEADER_GRAY)
                            .size(12.0),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .button("+")
                            .on_hover_text("新建示例（草稿）：保存时才落库")
                            .clicked()
                        {
                            self.select(None);
                        }
                    });
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(2.0);

                // 快照一次：遍历期间可能触发选中/新建，避免与 store 的可变借用冲突。
                // 草稿行固定在列表最上（id 为 None），它不在 store.rows() 里（§8）。
                let current_id = self.current_id();
                let images_dir = self.store.images_dir();
                let mut rows: Vec<SidebarRow> = Vec::with_capacity(self.store.row_count() + 1);
                if let Some(example) = self.current.as_ref().filter(|example| example.is_draft()) {
                    let thumb = match (&self.pending_image, example.image_file_name()) {
                        (Some(pending), _) => Some(ThumbSource::Pending {
                            key: pending.key.clone(),
                            path: pending.path.clone(),
                        }),
                        (None, Some(file_name)) => Some(ThumbSource::Managed(file_name)),
                        (None, None) => None,
                    };
                    rows.push(SidebarRow {
                        id: None,
                        title: example.display_title().to_string(),
                        thumb,
                    });
                }
                for row in self.store.rows() {
                    rows.push(SidebarRow {
                        id: Some(row.id),
                        title: row.display_title().to_string(),
                        thumb: row.image_file_name().map(ThumbSource::Managed),
                    });
                }

                let mut clicked: Option<Option<i64>> = None;
                let mut edit_start: Option<Option<i64>> = None;
                let mut edit_result: Option<InlineEditResult> = None;
                let mut insert_at: Option<(i64, usize)> = None;
                let mut drag_stopped = false;
                // 收集每一行的矩形：落点要相对所有行判断，而 dragged() 只在被拖那一行为真。
                // 草稿行不参与重排，因此不进 row_rects（下标也要与 store.rows() 对齐）。
                let mut row_rects: Vec<Rect> = Vec::with_capacity(rows.len());

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for row in rows.iter() {
                            let is_draft = row.id.is_none();
                            let selected = row.id == current_id;
                            let width = (ui.available_width() - 8.0).max(60.0);
                            let (rect, response) = ui.allocate_exact_size(
                                Vec2::new(width, 52.0),
                                Sense::click_and_drag(),
                            );
                            if !is_draft {
                                row_rects.push(rect);
                            }

                            // 标题子矩形的命中判定；绘制与判定共用同一函数，保证一致（§2）。
                            let text_rect = sidebar_row_text_rect(rect);
                            let pointer_on_title = ui
                                .ctx()
                                .pointer_hover_pos()
                                .map_or(false, |pos| text_rect.contains(pos));
                            let editing_this = self
                                .editing_title
                                .as_ref()
                                .map_or(false, |edit| edit.target == row.id);

                            if is_draft {
                                if response.clicked() && !editing_this {
                                    clicked = Some(row.id);
                                }
                            } else {
                                // 长按判定：按住超过 REORDER_HOLD 且产生位移，才进入重排。
                                if response.drag_started() {
                                    self.drag_row_id = row.id;
                                    self.drag_started = Instant::now();
                                    self.drag_moved = false;
                                    self.drag_target = None;
                                }
                                if response.dragged()
                                    && response.drag_delta().length() > REORDER_SLOP
                                {
                                    self.drag_moved = true;
                                }
                                if response.drag_stopped() {
                                    drag_stopped = true;
                                }
                                if response.clicked() && !self.drag_moved && !editing_this {
                                    // 方案 A（§2）：短按任意行选中；短按已选中行的标题文本进入内联编辑。
                                    if selected && pointer_on_title {
                                        edit_start = Some(row.id);
                                    } else {
                                        clicked = Some(row.id);
                                    }
                                }
                            }

                            // 当前行悬停标题：光标变 I-beam，右侧出现小铅笔（可发现性，§2）。
                            if selected && pointer_on_title && !editing_this {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                            }

                            paint_sidebar_row(
                                ui,
                                rect,
                                selected,
                                &response,
                                &row.title,
                                row.thumb.as_ref(),
                                &images_dir,
                                &mut self.cache,
                                is_draft,
                                editing_this,
                                selected && pointer_on_title && !editing_this,
                            );

                            if editing_this {
                                let edit = self
                                    .editing_title
                                    .as_mut()
                                    .expect("editing_this 蕴含编辑状态存在");
                                let response = ui.put(
                                    text_rect,
                                    TextEdit::singleline(&mut edit.buffer)
                                        .id(Id::new(("title_inline", row.id)))
                                        .desired_width(text_rect.width())
                                        .font(egui::FontId::proportional(13.5)),
                                );
                                if edit.needs_focus {
                                    response.request_focus();
                                    edit.needs_focus = false;
                                } else if ui.input(|i| i.key_pressed(Key::Escape)) {
                                    // Esc 取消；必须排在 lost_focus 之前，否则会被当成确认。
                                    edit_result = Some(InlineEditResult::Cancel);
                                } else if response.lost_focus() {
                                    // Enter 与失焦都确认。
                                    edit_result = Some(InlineEditResult::Confirm);
                                }
                            }
                        }
                        ui.add_space(4.0);

                        // 落点：按键针 y 与所有行的中心线求插入下标（指针上方有几行）。
                        if self.drag_row_id.is_some() && self.drag_moved {
                            let held = Instant::now().duration_since(self.drag_started);
                            if held >= REORDER_HOLD {
                                if let Some(pos) = ui.ctx().pointer_hover_pos() {
                                    let row_centers: Vec<f32> =
                                        row_rects.iter().map(|rect| rect.center().y).collect();
                                    self.drag_target = Some(drag_insertion_index(
                                        pos.y,
                                        ui.min_rect().bottom(),
                                        &row_centers,
                                    ));
                                }
                            }

                            // 长按拖拽时画一条插入位置指示线：画在真实落点的行边界上。
                            if let Some(insert) = self.drag_target {
                                let x_range = row_rects
                                    .first()
                                    .map(|rect| rect.x_range())
                                    .unwrap_or_else(|| ui.min_rect().x_range());
                                if let Some(y) = insertion_line_y(insert, &row_rects) {
                                    ui.painter().hline(
                                        x_range,
                                        y,
                                        Stroke::new(2.0, Color32::from_rgb(120, 170, 255)),
                                    );
                                }
                            }
                        }
                    });

                // 内联编辑收尾：确认/取消先于切换选中（失焦确认时可能同时点了别的行）。
                if let Some(target) = edit_start {
                    self.begin_inline_edit(target);
                }
                match edit_result {
                    Some(InlineEditResult::Confirm) => self.commit_inline_edit(),
                    Some(InlineEditResult::Cancel) => self.editing_title = None,
                    None => {}
                }
                if let Some(target) = clicked {
                    self.select(target);
                }

                if drag_stopped {
                    if let (Some(from), Some(insert)) = (self.drag_row_id, self.drag_target) {
                        if self.drag_moved {
                            insert_at = Some((from, insert));
                        }
                    }
                    self.drag_row_id = None;
                    self.drag_target = None;
                    self.drag_moved = false;
                }

                if let Some((from, insert)) = insert_at {
                    self.move_row(from, insert);
                }
            });
    }

    /// 开始内联编辑当前行的标题：点击标题文本或按 Ctrl+R（§2）。
    fn begin_inline_edit(&mut self, target: Option<i64>) {
        let Some(example) = self.current.as_ref() else {
            return;
        };
        if example.id != target {
            return;
        }
        self.editing_title = Some(InlineEdit {
            target,
            buffer: example.title.clone(),
            needs_focus: true,
        });
    }

    /// 确认标题内联编辑：已落库示例立即写库（同旧重命名弹窗），草稿只改内存（§2）。
    fn commit_inline_edit(&mut self) {
        let Some(edit) = self.editing_title.take() else {
            return;
        };
        if self.current_id() != edit.target {
            return;
        }
        let title = edit.buffer.trim().to_string();
        match edit.target {
            Some(id) => match self.store.load(id) {
                Ok(Some(mut stored)) => {
                    stored.title = title.clone();
                    match self.store.update(&mut stored) {
                        Ok(()) => {
                            // 只同步标题，保留其它未保存的编辑。
                            if let Some(current) = self.current.as_mut() {
                                current.title = title;
                            }
                            self.baseline = Some(stored);
                            self.status = "已重命名".to_string();
                            self.status_time = f64::NAN;
                        }
                        Err(err) => self.set_error(err),
                    }
                }
                Ok(None) => {}
                Err(err) => self.set_error(err),
            },
            None => {
                if let Some(current) = self.current.as_mut() {
                    current.title = title;
                }
            }
        }
    }

    /// 长按拖拽重排：把 `from` 移到 `insert` 位置并落库。
    fn move_row(&mut self, from: i64, insert: usize) {
        let mut order: Vec<i64> = self.store.rows().iter().map(|row| row.id).collect();
        let Some(source) = order.iter().position(|id| *id == from) else {
            return;
        };
        let Some(destination) = reorder_destination(source, insert, order.len()) else {
            return;
        };
        let moved = order.remove(source);
        order.insert(destination, moved);
        if let Err(err) = self.store.reorder(&order) {
            self.set_error(err);
        }
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        let resp = egui::Panel::bottom("bottom_bar")
            .show(ui, |ui| {
                ui.add_space(4.0);
                let mut actions: Vec<Action> = Vec::new();
                let mut open_about = false;
                ui.horizontal(|ui| {
                    let draft = self
                        .current
                        .as_ref()
                        .map(|example| example.is_draft())
                        .unwrap_or(true);
                    let dirty = self.dirty();
                    if draft {
                        if ui
                            .add_enabled(self.current.is_some(), egui::Button::new("保存为新示例"))
                            .on_hover_text("草稿落库：此时才把图片复制入库")
                            .clicked()
                        {
                            actions.push(Action::Save);
                        }
                    } else if ui
                        .add_enabled(dirty, egui::Button::new("保存"))
                        .on_hover_text("Ctrl+S")
                        .clicked()
                    {
                        actions.push(Action::Save);
                    }
                    if ui
                        .add_enabled(self.current.is_some(), egui::Button::new("另存为"))
                        .on_hover_text("复制成一条新示例，原示例保持不变（Ctrl+Shift+S）")
                        .clicked()
                    {
                        actions.push(Action::SaveAs);
                    }
                    if ui
                        .add_enabled(self.current_id().is_some(), egui::Button::new("删除"))
                        .clicked()
                    {
                        actions.push(Action::Delete);
                    }

                    ui.separator();
                    // §4：复制提示词沿用禁用逻辑——五个分区正文全空时置灰。
                    let clipboard_text = self
                        .current
                        .as_ref()
                        .map(|example| example.sections.join_for_clipboard())
                        .unwrap_or_default();
                    let can_copy = !clipboard_text.is_empty();
                    if ui
                        .add_enabled(can_copy, egui::Button::new("复制提示词"))
                        .on_hover_text("只复制五个分区的正文，不含分区标题")
                        .clicked()
                    {
                        ui.ctx().copy_text(clipboard_text);
                        self.set_status(ui.ctx(), "正向提示词已复制到剪贴板".to_string());
                    }
                    if ui.button("导出图库…").clicked() {
                        actions.push(Action::Export);
                    }
                    if ui.button("导入图库…").clicked() {
                        actions.push(Action::Import);
                    }
                    if ui.button("打开数据目录").clicked() {
                        actions.push(Action::OpenDataDir);
                    }

                    ui.separator();
                    let status_text = self.status_message(ui.ctx());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // 最右：版本按钮，点开「关于」（§5）。
                        let version = env!("CARGO_PKG_VERSION");
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!("v{version}"))
                                        .color(HEADER_GRAY)
                                        .size(12.0),
                                )
                                .frame(false),
                            )
                            .on_hover_text("关于 Prompt Gallery")
                            .clicked()
                        {
                            open_about = true;
                        }
                        // 状态文案占据剩余宽度，窄窗口下自适应截断（§4）。
                        let width = ui.available_width().max(0.0);
                        let height = ui.text_style_height(&egui::TextStyle::Body).max(16.0);
                        ui.add_sized(
                            [width, height],
                            egui::Label::new(
                                RichText::new(status_text).color(HEADER_GRAY).size(12.0),
                            )
                            .truncate(),
                        );
                    });
                });
                ui.add_space(4.0);
                if open_about {
                    self.about_open = true;
                }
                actions
            });
        let actions = resp.inner;

        for action in actions {
            match action {
                Action::Save => self.save_current(),
                Action::SaveAs => self.save_as(),
                Action::Delete => self.delete_confirm = true,
                Action::Export => {
                    self.export_path = Path::new(&self.data_dir)
                        .join(archive::default_file_name(now_unix()))
                        .to_string_lossy()
                        .to_string();
                    self.export_open = true;
                }
                Action::Import => self.import_dialog(ui.ctx()),
                Action::OpenDataDir => open_in_explorer(Path::new(&self.data_dir)),
            }
        }
    }

    /// 状态文案：过一段时间自动淡出，避免长期占位。
    fn status_message(&self, ctx: &egui::Context) -> String {
        if self.status.is_empty() {
            return String::new();
        }
        let now = ctx.input(|i| i.time);
        let age = now - self.status_time;
        if self.status_time != 0.0 && age > STATUS_TTL {
            return String::new();
        }
        self.status.clone()
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let total_height = ui.available_height();
            let image_height = (total_height * 0.42).clamp(140.0, 420.0);

            // 图片框
            self.image_box(ui, image_height);
            ui.add_space(6.0);
            // 提示词框：5 个 TextEdit 共用一个滚动条，视觉上是一个框（ADR-0002）
            self.prompt_editor(ui);
        });
    }

    fn image_box(&mut self, ui: &mut egui::Ui, height: f32) {
        let width = ui.available_width();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        let hovered = response.hovered();
        let pointer_over = ui
            .ctx()
            .pointer_hover_pos()
            .map_or(false, |pos| rect.contains(pos));
        // 拖入文件时高亮整框（§1）。
        let file_drop_hover =
            ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) && pointer_over;
        let stroke_color = if file_drop_hover {
            Color32::from_rgb(120, 170, 255)
        } else if hovered {
            Color32::from_gray(120)
        } else {
            Color32::from_gray(72)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(6),
            IMAGE_BOX_FILL,
            Stroke::new(
                if hovered || file_drop_hover { 2.0 } else { 1.0 },
                stroke_color,
            ),
            StrokeKind::Inside,
        );
        if hovered {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        // 右上角「×」：只解除引用，不删托管文件（§1、ADR-0004）。
        let remove_rect = Rect::from_min_size(
            egui::pos2(rect.right() - 26.0, rect.top() + 6.0),
            Vec2::splat(20.0),
        );
        let remove = ui
            .interact(remove_rect, Id::new("image_remove"), Sense::click())
            .on_hover_text("移除图片（托管文件保留）");
        if remove.hovered() || remove.clicked() {
            ui.painter().rect_filled(
                remove_rect,
                CornerRadius::same(4),
                Color32::from_rgba_unmultiplied(255, 255, 255, 24),
            );
            ui.painter().text(
                remove_rect.center(),
                egui::Align2::CENTER_CENTER,
                "×",
                egui::FontId::proportional(14.0),
                Color32::from_gray(210),
            );
        }

        let mut hover_text: Option<String> = None;
        if remove.clicked() {
            self.pending_image = None;
            if let Some(example) = self.current.as_mut() {
                example.image_hash = None;
                example.image_ext = None;
            }
            self.cache.drop_preview();
            self.set_status(ui.ctx(), "已移除图片引用（托管文件保留）".to_string());
        } else if response.clicked() {
            // 整框可点：有无图片都打开文件对话框（§1）。
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("图片", &IMAGE_EXTENSIONS)
                .pick_file()
            {
                self.set_pending_image(ui.ctx(), &path);
            }
        }

        // 待入库图片（只记源路径）优先于已入库图片；尚未保存时也能看到预览（ADR-0004）。
        let pending = self.pending_image.clone();
        let managed = self
            .current
            .as_ref()
            .filter(|example| example.is_draft() || example.image_hash.is_some())
            .and_then(|example| example.image_file_name());
        let images_dir = self.store.images_dir();
        match (&pending, &managed) {
            (Some(pending), _) => {
                hover_text = Some(format!(
                    "待入库：{}（保存时按内容哈希写入 images/）",
                    pending.label
                ));
                match self.cache.preview(ui.ctx(), &pending.key, &pending.path) {
                    Ok(cached) => {
                        let target = fit_size(cached.aspect(), rect.size() - Vec2::splat(16.0));
                        let target = Rect::from_center_size(rect.center(), target);
                        egui::Image::new(&cached.texture)
                            .fit_to_exact_size(target.size())
                            .paint_at(ui, target);
                    }
                    Err(err) => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("图片无法显示{}", err),
                            egui::FontId::proportional(13.0),
                            Color32::from_gray(200),
                        );
                    }
                }
            }
            (None, Some(key)) => {
                if let Some(hash) = self
                    .current
                    .as_ref()
                    .and_then(|example| example.image_hash.clone())
                {
                    let short: String = hash.chars().take(8).collect();
                    hover_text = Some(format!("内容哈希：{}…", short));
                }
                match self.cache.preview(ui.ctx(), key, &images_dir.join(key)) {
                    Ok(cached) => {
                        let target = fit_size(cached.aspect(), rect.size() - Vec2::splat(16.0));
                        let target = Rect::from_center_size(rect.center(), target);
                        egui::Image::new(&cached.texture)
                            .fit_to_exact_size(target.size())
                            .paint_at(ui, target);
                    }
                    Err(err) => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("图片无法显示{}", err),
                            egui::FontId::proportional(13.0),
                            Color32::from_gray(200),
                        );
                    }
                }
            }
            (None, None) => {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "点击或拖拽本地图片到此处",
                    egui::FontId::proportional(14.0),
                    Color32::from_gray(150),
                );
            }
        }

        // 已有图片时叠一层半透明提示，鼠标移入才显示「点击更换图片」（§1）。
        if hovered && (pending.is_some() || managed.is_some()) {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(6),
                Color32::from_rgba_unmultiplied(0, 0, 0, 90),
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "点击更换图片",
                egui::FontId::proportional(14.0),
                Color32::from_gray(235),
            );
        }

        if let Some(text) = hover_text {
            response.on_hover_text(text);
        }
    }

    /// 提示词编辑器：5 个无缝拼接的 TextEdit（ADR-0002）。
    ///
    /// 分区标题是独立的灰字 `Label`（`selectable(false)`），不是文本框内容的一部分，
    /// 因此天然不可编辑、不可选中，也不会随复制进入剪贴板。
    fn prompt_editor(&mut self, ui: &mut egui::Ui) {
        let Some(example) = self.current.as_mut() else {
            ui.label(RichText::new("没有选中的示例").color(HEADER_GRAY));
            return;
        };

        Frame::new()
            .fill(Color32::from_gray(20))
            .stroke(Stroke::new(1.0, Color32::from_gray(70)))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ScrollArea::vertical()
                    .id_salt("prompt_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // 铺满编辑区宽度，让五个分区看起来是一个框的内部分栏。
                        ui.set_min_width(ui.available_width());
                        for index in 0..SECTION_COUNT {
                            section_editor(ui, &mut example.sections, index);
                        }
                        ui.add_space(4.0);
                    });
            });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        // 未保存改动确认。
        if self.pending_select.is_some() {
            egui::Modal::new(Id::new("unsaved_modal")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.heading("有未保存的改动");
                ui.add_space(6.0);
                ui.label("当前示例的改动还没有保存，要如何处理？");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("保存并继续").clicked() {
                        self.save_current();
                        self.perform_pending_select();
                    }
                    if ui.button("放弃改动").clicked() {
                        self.perform_pending_select();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("取消").clicked() {
                            self.pending_select = None;
                        }
                    });
                });
            });
        }

        // 关于（§5：底栏最右的版本按钮打开）。
        if self.about_open {
            let mut close = false;
            egui::Modal::new(Id::new("about_modal")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading(format!("{APP_TITLE} v{}", env!("CARGO_PKG_VERSION")));
                ui.add_space(8.0);
                ui.label(RichText::new("数据目录").color(HEADER_GRAY).size(12.0));
                ui.monospace(self.data_dir.clone());
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!("归档格式版本 {}", archive::ARCHIVE_VERSION))
                        .color(HEADER_GRAY)
                        .size(12.0),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("复制路径").clicked() {
                        ui.ctx().copy_text(self.data_dir.clone());
                        self.set_status(ui.ctx(), "数据目录路径已复制".to_string());
                    }
                    if ui.button("打开数据目录").clicked() {
                        open_in_explorer(Path::new(&self.data_dir));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("关闭").clicked() {
                            close = true;
                        }
                    });
                });
            });
            if close {
                self.about_open = false;
            }
        }

        // 删除确认。
        if self.delete_confirm {
            let mut close = false;
            let mut confirm = false;
            egui::Modal::new(Id::new("delete_modal")).show(ctx, |ui| {
                ui.set_width(340.0);
                ui.heading("删除这条示例？");
                ui.add_space(6.0);
                let name = self
                    .current
                    .as_ref()
                    .map(|example| example.display_title().to_string())
                    .unwrap_or_default();
                ui.label(format!("「{name}」的标题与提示词将被删除。"));
                ui.label(
                    RichText::new("托管目录里的图片文件会保留（可能被其它示例共用）。")
                        .color(HEADER_GRAY)
                        .size(12.0),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("删除").clicked() {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                });
            });
            if confirm {
                self.delete_current();
                close = true;
            }
            if close {
                self.delete_confirm = false;
            }
        }

        // 导出。
        if self.export_open {
            let mut close = false;
            let mut do_export = false;
            let mut browse = false;
            egui::Modal::new(Id::new("export_modal")).show(ctx, |ui| {
                ui.set_width(520.0);
                ui.heading("导出图库");
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "把 {} 条示例导出为 JSON + 图片文件的 zip 包。",
                        self.store.row_count()
                    ))
                    .color(HEADER_GRAY)
                    .size(12.0),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(
                        TextEdit::singleline(&mut self.export_path)
                            .desired_width(360.0)
                            .hint_text("导出路径"),
                    );
                    browse = ui.button("浏览…").clicked();
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("导出").clicked() {
                        do_export = true;
                    }
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                });
            });

            if browse {
                let start = Path::new(&self.export_path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| archive::default_file_name(now_unix()));
                let picked = rfd::FileDialog::new()
                    .add_filter("zip", &["zip"])
                    .set_file_name(start)
                    .set_directory(&self.data_dir)
                    .save_file();
                if let Some(path) = picked {
                    self.export_path = path.to_string_lossy().to_string();
                }
            } else if do_export {
                self.export_to(ctx);
            } else if close {
                self.export_open = false;
            }
        }
    }
}

/// 底部按钮触发的动作，由闭包收集后在外层统一执行（避免与 self 的借用纠缠）。
#[derive(Clone, Copy)]
enum Action {
    Save,
    SaveAs,
    Delete,
    Export,
    Import,
    OpenDataDir,
}

// ---------------------------------------------------------------- 自定义控件

/// 分区编辑器：一个灰色不可选中的标题 + 一个无边框 TextEdit。
fn section_editor(ui: &mut egui::Ui, sections: &mut Sections, index: SectionIndex) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        // 标题：灰色、左对齐、不可修改、不可选中。
        //
        // 用 painter 直接排版而不是 `Label`：行高留出富余并垂直居中，
        // 灰色标题与下面的文本框之间也因此有稳定的间距。
        let font = egui::FontId::proportional(12.0);
        let row_height = ui.ctx().fonts_mut(|fonts| fonts.row_height(&font));
        let galley =
            ui.painter()
                .layout_no_wrap(SECTION_TITLES[index].to_string(), font, HEADER_GRAY);
        let head_height = row_height.max(galley.size().y) + 6.0;
        let (head_rect, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), head_height),
            Sense::hover(),
        );
        let text_pos = egui::pos2(
            head_rect.left(),
            head_rect.center().y - galley.size().y * 0.5,
        );
        ui.painter().galley(text_pos, galley, HEADER_GRAY);

        let width = ui.available_width();
        let text = sections
            .get_mut(index)
            .expect("分区下标由 SECTION_COUNT 约束");
        ui.add(
            TextEdit::multiline(text)
                .id(Id::new(("section_edit", index)))
                .desired_width(width)
                .desired_rows(2)
                .frame(Frame::NONE)
                .hint_text("（留空）"),
        );

        // 分区之间的细分隔线：让五个文本框看起来是一个框的内部分栏，
        // 而不是五个独立控件（ADR-0002）。
        if index + 1 < SECTION_COUNT {
            ui.add_space(3.0);
            let (line_rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
            ui.painter().hline(
                line_rect.x_range(),
                line_rect.center().y,
                Stroke::new(1.0, SEAM_COLOR),
            );
            ui.add_space(4.0);
        }
    });
}

// ---------------------------------------------------------------- 侧边栏行

/// 待入库图片（Pending Image）：只记源路径，保存时才按内容哈希写入托管目录（ADR-0004）。
#[derive(Clone)]
struct PendingImage {
    path: PathBuf,
    /// 源文件名，仅用于状态与悬停提示。
    label: String,
    /// 纹理缓存 key；同一张图在多次重绘间保持稳定。
    key: String,
}

/// 侧边栏标题的内联编辑状态（§2）。
struct InlineEdit {
    /// 正在编辑的示例 id；None 表示草稿行。
    target: Option<i64>,
    buffer: String,
    /// 首次显示时请求键盘焦点。
    needs_focus: bool,
}

/// 内联编辑的收尾方式。
#[derive(Clone, Copy)]
enum InlineEditResult {
    Confirm,
    Cancel,
}

/// 侧边栏一行的快照：草稿行的 id 为 None，它不在 store.rows() 里（§8）。
struct SidebarRow {
    id: Option<i64>,
    title: String,
    thumb: Option<ThumbSource>,
}

/// 侧边栏缩略图的来源（快照，避免与 store 的可变借用冲突）。
enum ThumbSource {
    /// 托管目录里的文件（images/hash.ext）。
    Managed(String),
    /// 待入库图片：缓存 key + 源路径。
    Pending { key: String, path: PathBuf },
}

/// 画一整行侧边栏条目：缩略图 + 标题，选中态高亮。
fn paint_sidebar_row(
    ui: &mut egui::Ui,
    rect: Rect,
    selected: bool,
    response: &egui::Response,
    title: &str,
    thumb: Option<&ThumbSource>,
    images_dir: &Path,
    cache: &mut ImageCache,
    draft: bool,
    editing: bool,
    show_pencil: bool,
) {
    let bg = if selected {
        Color32::from_rgb(48, 62, 88)
    } else if response.hovered() {
        Color32::from_gray(46)
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(5), bg);

    // 缩略图
    let thumb_rect = sidebar_row_thumb_rect(rect);
    let mut drew_thumb = false;
    if let Some(source) = thumb {
        let decoded = match source {
            ThumbSource::Managed(file_name) => file_name
                .split('.')
                .next()
                .map(|key| cache.thumbnail(ui.ctx(), key, &images_dir.join(file_name))),
            ThumbSource::Pending { key, path } => Some(cache.thumbnail(ui.ctx(), key, path)),
        };
        if let Some(Ok(cached)) = decoded {
            let size = fit_size(cached.aspect(), thumb_rect.size());
            let target = Rect::from_center_size(thumb_rect.center(), size);
            egui::Image::new(&cached.texture)
                .fit_to_exact_size(size)
                .paint_at(ui, target);
            drew_thumb = true;
        }
    }
    if !drew_thumb {
        ui.painter()
            .rect_filled(thumb_rect, CornerRadius::same(4), Color32::from_gray(58));
        ui.painter().text(
            thumb_rect.center(),
            egui::Align2::CENTER_CENTER,
            "无图",
            egui::FontId::proportional(11.0),
            Color32::from_gray(150),
        );
    }

    let text_rect = sidebar_row_text_rect(rect);

    // 标题：用 painter 直接画，缩略图与标题因此可以在同一行精确对齐。
    // 内联编辑时标题交给 TextEdit 画，这里跳过。
    if !editing {
        let galley = ui.painter().layout(
            title.to_string(),
            egui::FontId::proportional(13.5),
            if selected {
                Color32::from_gray(238)
            } else {
                Color32::from_gray(210)
            },
            text_rect.width(),
        );
        ui.painter().galley(
            egui::pos2(text_rect.left(), text_rect.top()),
            galley,
            Color32::PLACEHOLDER,
        );
    }

    // 当前行悬停标题：右侧的小铅笔，提示「点标题可改名」（§2 可发现性）。
    if show_pencil {
        ui.painter().text(
            egui::pos2(text_rect.right() - 2.0, text_rect.top() + 7.0),
            egui::Align2::RIGHT_CENTER,
            "✎",
            egui::FontId::proportional(12.0),
            Color32::from_gray(170),
        );
    }

    // 草稿标记（§8）。
    if draft {
        ui.painter().text(
            egui::pos2(rect.right() - 6.0, rect.bottom() - 5.0),
            egui::Align2::RIGHT_BOTTOM,
            "草稿",
            egui::FontId::proportional(10.0),
            Color32::from_gray(150),
        );
    }
}

/// 侧边栏一行里缩略图所占的子矩形。
fn sidebar_row_thumb_rect(rect: Rect) -> Rect {
    Rect::from_min_size(
        rect.min + Vec2::new(6.0, 6.0),
        Vec2::splat(rect.height() - 12.0),
    )
}

/// 侧边栏一行里标题所占的子矩形；绘制与点击命中判定共用，保证一致（§2）。
fn sidebar_row_text_rect(rect: Rect) -> Rect {
    let thumb = sidebar_row_thumb_rect(rect);
    Rect::from_min_max(
        egui::pos2(thumb.right() + 8.0, rect.top() + 6.0),
        egui::pos2(rect.right() - 6.0, rect.bottom() - 6.0),
    )
}

// ---------------------------------------------------------------- 重排落点

/// 依据指针 y 与各行中心线求出**插入下标**：语义是「指针上方（中心线以上）的行数」，
/// 取值恒在 `0..=row_centers.len()` 内。
///
/// 与中心线齐平时算作「在其上方」，与旧单行判定的 `pos.y >= rect.center().y` 保持一致；
/// 于是被拖行自身占据的区间自然成为空操作区间（插到自己前面或后面都不改变顺序）。
fn insertion_index(pointer_y: f32, row_centers: &[f32]) -> usize {
    row_centers
        .iter()
        .filter(|center| pointer_y >= **center)
        .count()
}

/// 拖拽落点：按键针 y 与所有行的中心线求插入下标。
///
/// `list_bottom_y` 是列表区域的底边；指针落到它下方即视为「插到末尾」——保留原有语义，
/// 也是「一次把行拖到列表最后」最省事的手势。
fn drag_insertion_index(pointer_y: f32, list_bottom_y: f32, row_centers: &[f32]) -> usize {
    if pointer_y > list_bottom_y {
        row_centers.len()
    } else {
        insertion_index(pointer_y, row_centers)
    }
}

/// 把插入下标换算成「移除源元素后」应插入的下标；返回 `None` 表示原地松手、顺序不变。
///
/// `insert` 是 [`insertion_index`] 的结果（`0..=len`）：先按原数组夹取到 `len`，
/// 若落在源元素之后则减一（原数组里插在其后 == 移除后插在其所在位），最后与源下标比较。
fn reorder_destination(source: usize, insert: usize, len: usize) -> Option<usize> {
    let destination = insert.min(len);
    let destination = if destination > source {
        destination - 1
    } else {
        destination
    };
    (destination != source).then_some(destination)
}

/// 插入指示线应画的 y 坐标：落在插入处的两行边界上；首行之上 / 末行之下各留 1pt，
/// 让线不至于完全压住行边框。列表为空时没有可画的位置。
fn insertion_line_y(insert: usize, row_rects: &[Rect]) -> Option<f32> {
    let first = row_rects.first()?;
    let last = row_rects.last()?;
    Some(if insert == 0 {
        first.top() - 1.0
    } else if insert >= row_rects.len() {
        last.bottom() + 1.0
    } else {
        // 相邻两行之间（行距可能大于 0），取缝隙中线。
        (row_rects[insert - 1].bottom() + row_rects[insert].top()) / 2.0
    })
}

// ---------------------------------------------------------------- 字体与图标

/// 挂上 Windows 自带的 CJK 字体，保证中文界面不出现豆腐块。
///
/// egui 自带的 `default_fonts` 只覆盖拉丁字形，因此中文字体作为**回退**追加在
/// 拉丁字体之后：英文仍用 egui 的默认字形，中文落到微软雅黑。
fn install_cjk_fonts(ctx: &egui::Context) {
    const CANDIDATES: [&str; 4] = [
        "C:/Windows/Fonts/msyh.ttc",
        "C:/Windows/Fonts/msyh.ttf",
        "C:/Windows/Fonts/simhei.ttf",
        "C:/Windows/Fonts/simsun.ttc",
    ];
    let Some(bytes) = CANDIDATES.iter().find_map(|path| std::fs::read(path).ok()) else {
        eprintln!("警告：未找到系统中文字体，界面中文可能显示为方块");
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk".to_owned(),
        std::sync::Arc::new(egui::FontData::from_owned(bytes)),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let list = fonts.families.entry(family).or_default();
        if !list.iter().any(|name| name == "cjk") {
            list.push("cjk".to_owned());
        }
    }
    ctx.set_fonts(fonts);
}

/// 程序图标：深色底 + 白色「图片框」 + 五条分区线，示意「一张图 + 五段提示词」。
fn app_icon() -> egui::IconData {
    const SIZE: usize = 64;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let i = (y * SIZE + x) * 4;
            // 竖向渐变底色
            let t = y as f32 / (SIZE - 1) as f32;
            let base = (40.0 + t * 24.0) as u8;
            rgba[i] = base;
            rgba[i + 1] = (base as f32 + 6.0) as u8;
            rgba[i + 2] = (base as f32 + 22.0) as u8;
            rgba[i + 3] = 255;
        }
    }
    // 上面一块「图片」占位框
    for y in 10..30 {
        for x in 10..54 {
            let i = (y * SIZE + x) * 4;
            rgba[i] = 210;
            rgba[i + 1] = 214;
            rgba[i + 2] = 220;
        }
    }
    // 下面五条横线代表五个分区
    for line in 0..5usize {
        let y0 = 36 + line * 5;
        for y in y0..(y0 + 2) {
            for x in 10..54 {
                let i = (y * SIZE + x) * 4;
                rgba[i] = 150;
                rgba[i + 1] = 170;
                rgba[i + 2] = 205;
            }
        }
    }
    egui::IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}

/// 在资源管理器里打开目录。
fn open_in_explorer(path: &Path) {
    #[cfg(target_os = "windows")]
    {
        // 显式用 explorer.exe，避免 `start` 的引号解析问题。
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}

// ---------------------------------------------------------------- 单元测试

#[cfg(test)]
mod tests {
    use super::*;

    /// 四行列表的行中心线：52pt 行高 + egui 默认行距 3pt。
    fn centers4() -> Vec<f32> {
        vec![30.0, 85.0, 140.0, 195.0]
    }

    /// 按 `move_row` 的同一套算术模拟一次「拖拽松手」后的顺序：
    /// `insert` 为落点下标（指针上方有几行）。
    fn drag_release(ids: &[&str], source: usize, insert: usize) -> Vec<String> {
        let mut order: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        if let Some(destination) = reorder_destination(source, insert, order.len()) {
            let moved = order.remove(source);
            order.insert(destination, moved);
        }
        order
    }

    #[test]
    fn insertion_index_counts_rows_above_pointer() {
        let centers = centers4();
        assert_eq!(insertion_index(0.0, &centers), 0); // 首行中心之上
        assert_eq!(insertion_index(29.9, &centers), 0);
        assert_eq!(insertion_index(30.0, &centers), 1); // 与首行中心齐平：算在其上方
        assert_eq!(insertion_index(57.0, &centers), 1); // 第 1、2 行之间
        assert_eq!(insertion_index(112.0, &centers), 2); // 第 2、3 行之间
        assert_eq!(insertion_index(167.5, &centers), 3); // 第 3、4 行之间
        assert_eq!(insertion_index(195.0, &centers), 4); // 与末行中心齐平
        assert_eq!(insertion_index(1000.0, &centers), 4); // 末行中心之下 => 末尾
        assert_eq!(insertion_index(0.0, &[]), 0); // 空列表
    }

    #[test]
    fn insertion_index_never_exceeds_row_count() {
        let centers = centers4();
        for y in [-500.0, 0.0, 30.0, 500.0, 10_000.0] {
            assert!(insertion_index(y, &centers) <= centers.len());
        }
    }

    #[test]
    fn drag_insertion_index_keeps_below_list_means_end() {
        let centers = centers4();
        let list_bottom = 240.0; // 列表底边（末行 bottom 217 之下）
        assert_eq!(drag_insertion_index(100.0, list_bottom, &centers), 2);
        assert_eq!(drag_insertion_index(230.0, list_bottom, &centers), 4);
        assert_eq!(drag_insertion_index(240.0, list_bottom, &centers), 4); // 正好在底边：不算「下方」
        assert_eq!(drag_insertion_index(241.0, list_bottom, &centers), 4); // 落到列表下方 => 末尾
        assert_eq!(drag_insertion_index(9999.0, list_bottom, &centers), 4);
        assert_eq!(drag_insertion_index(50.0, list_bottom, &[]), 0); // 空列表
        assert_eq!(drag_insertion_index(9999.0, list_bottom, &[]), 0);
    }

    #[test]
    fn insertion_line_sits_on_real_gap() {
        let rows: Vec<Rect> = (0..4)
            .map(|index| {
                let top = index as f32 * 55.0;
                Rect::from_min_size(egui::pos2(0.0, top), Vec2::new(200.0, 52.0))
            })
            .collect();
        // 首行之上 / 末行之下（含 insert == row_count）：各留 1pt。
        // 末行 top = 3 * 55 = 165、bottom = 217，故末尾线在 218。
        assert_eq!(insertion_line_y(0, &rows), Some(-1.0));
        assert_eq!(insertion_line_y(4, &rows), Some(218.0));
        assert_eq!(insertion_line_y(99, &rows), Some(218.0)); // 越界也夹到末尾
        // 中间落在两行缝隙的中线上，而不是被拖行自己的边缘。
        assert_eq!(insertion_line_y(1, &rows), Some((52.0 + 55.0) / 2.0));
        assert_eq!(insertion_line_y(2, &rows), Some((107.0 + 110.0) / 2.0));
        assert_eq!(insertion_line_y(3, &rows), Some((162.0 + 165.0) / 2.0));
        // 空列表没有可画的位置。
        assert_eq!(insertion_line_y(0, &[]), None);
    }

    /// 验收里点名的五条边界：第一行拖到最后、最后一行拖到最前、向下移一位、
    /// 向上移一位、原地松手。
    #[test]
    fn drag_edges_reorder_as_expected() {
        let ids = ["A", "B", "C", "D"];

        // 第一行拖到最后（指针落到列表下方 => insert = row_count = 4）。
        assert_eq!(drag_release(&ids, 0, 4), ["B", "C", "D", "A"]);
        // 最后一行拖到最前（指针在首行中心之上 => insert = 0）。
        assert_eq!(drag_release(&ids, 3, 0), ["D", "A", "B", "C"]);
        // 向下移一位：A 落到 B、C 之间 => insert = 2。
        assert_eq!(drag_release(&ids, 0, 2), ["B", "A", "C", "D"]);
        // 向上移一位：C 落到 A、B 之间 => insert = 1。
        assert_eq!(drag_release(&ids, 2, 1), ["A", "C", "B", "D"]);
        // 原地松手：插到自己前面/后面都不改变顺序。
        for source in 0..ids.len() {
            assert_eq!(drag_release(&ids, source, source), ids.map(str::to_string));
            assert_eq!(
                drag_release(&ids, source, source + 1),
                ids.map(str::to_string)
            );
        }
    }

    /// 把「指针位置」和「落点下标」串起来，验证被拖行自身区间是空操作区间。
    #[test]
    fn pointer_within_dragged_row_is_noop() {
        let ids = ["A", "B", "C", "D"];
        let centers = centers4();

        // 指针停在被拖行自己的中心附近：插到自己前面或后面 => 空操作。
        assert_eq!(
            drag_release(&ids, 1, insertion_index(centers[1], &centers)),
            ids.map(str::to_string)
        );
        assert_eq!(
            drag_release(&ids, 1, insertion_index(centers[1] + 20.0, &centers)),
            ids.map(str::to_string)
        );

        // 拖到末行中心之下：B 落到末尾。
        assert_eq!(
            drag_release(&ids, 1, insertion_index(1000.0, &centers)),
            ["A", "C", "D", "B"]
        );
        // 拖到首行中心之上：D 落到最前。
        assert_eq!(
            drag_release(&ids, 3, insertion_index(0.0, &centers)),
            ["D", "A", "B", "C"]
        );
        // 已经在第一个，再往上拖仍是空操作。
        assert_eq!(
            drag_release(&ids, 0, insertion_index(0.0, &centers)),
            ids.map(str::to_string)
        );
    }

    /// `insert` 越界（例如指针落到列表下方给的是 `row_count`）必须被夹住，不能 panic。
    #[test]
    fn reorder_destination_clamps_out_of_range_insert() {
        assert_eq!(reorder_destination(0, 4, 4), Some(3));
        assert_eq!(reorder_destination(0, 999, 4), Some(3));
        assert_eq!(reorder_destination(0, 0, 4), None);
        assert_eq!(reorder_destination(0, 1, 4), None);
        assert_eq!(reorder_destination(3, 0, 4), Some(0));
        assert_eq!(reorder_destination(3, 3, 4), None);
        assert_eq!(reorder_destination(3, 4, 4), None);
    }

    // ------------------------------------------------------------ 0.1.1 新增

    /// 临时数据目录；Drop 时清理，测试 panic 也会清理。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            path.push(format!(
                "prompt-gallery-main-test-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 生成一张真实的 PNG，保证能被内容嗅探与解码器识别。
    fn sample_png() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn pending_at(path: &Path) -> PendingImage {
        PendingImage {
            path: path.to_path_buf(),
            label: path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default(),
            key: format!("pending:{}", path.display()),
        }
    }

    fn managed_file_count(store: &Store) -> usize {
        std::fs::read_dir(store.images_dir())
            .map(|entries| entries.count())
            .unwrap_or(0)
    }

    /// §6 边界：选图/拖图只记源路径，不写托管目录（ADR-0004）。
    #[test]
    fn selecting_an_image_leaves_the_managed_dir_empty() {
        let dir = TempDir::new("pending");
        let store = Store::open_in(dir.0.clone()).unwrap();
        let source = dir.0.join("source.png");
        std::fs::write(&source, sample_png()).unwrap();

        let pending = pending_at(&source);
        assert_eq!(pending.path, source);
        assert_eq!(managed_file_count(&store), 0);
    }

    /// §6：保存时才按内容哈希入库，文件内容与源文件一致。
    #[test]
    fn save_ingests_pending_image_by_content_hash() {
        let dir = TempDir::new("ingest");
        let store = Store::open_in(dir.0.clone()).unwrap();
        let source = dir.0.join("source.png");
        let bytes = sample_png();
        std::fs::write(&source, &bytes).unwrap();

        let (hash, ext) = ingest_pending_image(&store, &pending_at(&source)).unwrap();
        assert_eq!(ext, "png");
        let stored = store.images_dir().join(format!("{hash}.{ext}"));
        assert!(stored.is_file());
        assert_eq!(std::fs::read(&stored).unwrap(), bytes);
        assert_eq!(managed_file_count(&store), 1);
    }

    /// §6 边界 ①：源文件在保存前消失 → 报错，不静默丢图、也不留托管文件。
    #[test]
    fn save_fails_when_pending_source_disappeared() {
        let dir = TempDir::new("missing");
        let store = Store::open_in(dir.0.clone()).unwrap();
        let source = dir.0.join("gone.png");
        assert!(ingest_pending_image(&store, &pending_at(&source)).is_err());
        assert_eq!(managed_file_count(&store), 0);
    }

    /// §6 边界 ②：同样内容只入库一份（另存为时不重复写）。
    #[test]
    fn same_content_is_stored_once() {
        let dir = TempDir::new("dedupe");
        let store = Store::open_in(dir.0.clone()).unwrap();
        let bytes = sample_png();
        let a = dir.0.join("a.png");
        let b = dir.0.join("b.png");
        std::fs::write(&a, &bytes).unwrap();
        std::fs::write(&b, &bytes).unwrap();

        let first = ingest_pending_image(&store, &pending_at(&a)).unwrap();
        let second = ingest_pending_image(&store, &pending_at(&b)).unwrap();
        assert_eq!(first, second);
        assert_eq!(managed_file_count(&store), 1);
    }

    /// §8：草稿落库到列表顶部，导入仍追加到末尾（有意的不对称）。
    #[test]
    fn draft_saves_to_top_while_import_appends() {
        let dir = TempDir::new("top");
        let mut store = Store::open_in(dir.0.clone()).unwrap();
        let ids = |store: &Store| -> Vec<i64> {
            store.rows().iter().map(|row| row.id).collect()
        };

        let mut first = Example::draft(0);
        first.title = "A".to_string();
        let first_id = store.insert(&mut first).unwrap();
        let mut second = Example::draft(0);
        second.title = "B".to_string();
        let second_id = store.insert(&mut second).unwrap();
        assert_eq!(ids(&store), vec![first_id, second_id]);

        let mut draft = Example::draft(0);
        draft.title = "D".to_string();
        let draft_id = store.insert_at_top(&mut draft).unwrap();
        assert_eq!(draft.position, 0);
        assert_eq!(ids(&store), vec![draft_id, first_id, second_id]);

        let mut imported = Example::draft(0);
        imported.title = "E".to_string();
        let imported_id = store.insert(&mut imported).unwrap();
        assert_eq!(ids(&store), vec![draft_id, first_id, second_id, imported_id]);
    }

    /// 连续两次落库草稿：最新保存的始终在最上，且 position 保持连续。
    #[test]
    fn consecutive_drafts_stack_at_the_top() {
        let dir = TempDir::new("stack");
        let mut store = Store::open_in(dir.0.clone()).unwrap();

        let mut a = Example::draft(0);
        a.title = "A".to_string();
        let a_id = store.insert(&mut a).unwrap();
        let mut b = Example::draft(0);
        b.title = "B".to_string();
        let b_id = store.insert_at_top(&mut b).unwrap();
        let mut c = Example::draft(0);
        c.title = "C".to_string();
        let c_id = store.insert_at_top(&mut c).unwrap();

        let order: Vec<i64> = store.rows().iter().map(|row| row.id).collect();
        assert_eq!(order, vec![c_id, b_id, a_id]);
        let positions: Vec<i64> = order
            .iter()
            .map(|id| store.load(*id).unwrap().unwrap().position)
            .collect();
        assert_eq!(positions, vec![0, 1, 2]);
    }
}
