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
/// 「长按拖拽重排」的判定时间：按住这个时长之后才允许重排，短按仍然是「选中」。
const REORDER_HOLD: Duration = Duration::from_millis(200);
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
    /// 草稿选中、尚未落库的图片尺寸提示。
    pending_image_size: Option<(u32, u32)>,
    status: String,
    status_time: f64,
    /// 重命名弹窗。
    rename_open: bool,
    rename_buffer: String,
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
            pending_image_size: None,
            status: String::new(),
            status_time: f64::NEG_INFINITY,
            rename_open: false,
            rename_buffer: String::new(),
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
    fn dirty(&self) -> bool {
        match (&self.current, &self.baseline) {
            (Some(current), Some(baseline)) => current.differs_from(baseline),
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    fn load_example(&mut self, id: i64) {
        match self.store.load(id) {
            Ok(Some(example)) => {
                self.baseline = Some(example.clone());
                self.current = Some(example);
                self.pending_image_size = None;
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
        self.pending_image_size = None;
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

    /// 保存：草稿走 insert（此时才触发图片复制入库），已有示例走 update。
    fn save_current(&mut self) {
        let store = &mut self.store;
        let mut result: anyhow::Result<()> = Ok(());
        if let Some(example) = self.current.as_mut() {
            example.title = example.title.trim().to_string();
            result = match example.id {
                Some(_) => store.update(example),
                None => store.insert(example).map(|_| ()),
            };
        }
        match result {
            Ok(()) => {
                self.baseline = self.current.clone();
                self.pending_image_size = None;
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

    /// 把图片字节复制进托管 `images/` 目录，并挂到当前示例上。
    ///
    /// 扩展名按内容嗅探（`ext_for_bytes`）而不是信任文件名，避免把 jpg 存成 .png。
    fn embed_image_bytes(&mut self, ctx: &egui::Context, bytes: &[u8], label: &str) {
        let Some(ext) = store::ext_for_bytes(bytes) else {
            self.set_status(ctx, format!("无法识别的图片格式：{label}"));
            return;
        };
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            self.set_status(
                ctx,
                format!("图片过大（{} MB），已拒绝载入", bytes.len() / 1024 / 1024),
            );
            return;
        }
        match self.store.store_image_bytes(bytes, ext) {
            Ok((hash, ext)) => {
                let size = image_size_of(bytes);
                if let Some(example) = self.current.as_mut() {
                    example.image_hash = Some(hash);
                    example.image_ext = Some(ext);
                }
                self.pending_image_size = size;
                self.cache.drop_preview();
                let hint = match size {
                    Some((w, h)) => format!("已选择图片 {w}×{h}"),
                    None => "已选择图片".to_string(),
                };
                self.set_status(ctx, hint);
            }
            Err(err) => self.set_error(err),
        }
    }

    fn embed_image_file(&mut self, ctx: &egui::Context, path: &Path) {
        match std::fs::read(path) {
            Ok(bytes) => self.embed_image_bytes(ctx, &bytes, &path.display().to_string()),
            Err(err) => {
                let err = anyhow::Error::new(err)
                    .context(format!("读取图片失败：{}", path.display()));
                self.set_error(err);
            }
        }
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

/// 只读图片头取尺寸；失败返回 `None`（不阻塞保存流程）。
fn image_size_of(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

// ---------------------------------------------------------------- 界面

impl eframe::App for GalleryView {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // 拖进窗口的图片文件（与侧边栏重排不冲突：那是内部拖拽，这里是外部文件）。
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(file) = dropped.into_iter().next() {
            match file.bytes() {
                Ok(bytes) => {
                    let label = file.path().display().to_string();
                    self.embed_image_bytes(&ctx, &bytes, &label);
                }
                Err(err) => self.set_status(&ctx, format!("读取拖入的文件失败：{err}")),
            }
        }

        self.handle_shortcuts(&ctx);
        self.top_bar(ui);
        self.sidebar(ui);
        self.bottom_bar(ui);
        self.central(ui);
        self.dialogs(&ctx);
        self.tick_status(&ctx);
        self.window_title(&ctx);

        if self.rename_open || self.delete_confirm || self.pending_select.is_some() {
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
            if let Some(example) = &self.current {
                self.rename_buffer = example.title.clone();
                self.rename_open = true;
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("top_bar").show(ui, |ui| {
            ui.add_space(6.0);
            let mut pick_image = false;
            let mut clear_image = false;
            let mut open_data_dir = false;
            let mut import = false;
            let mut open_export = false;

            ui.horizontal(|ui| {
                ui.label(RichText::new("标题").color(HEADER_GRAY).size(12.0));
                if let Some(example) = self.current.as_mut() {
                    let response = ui.add(
                        TextEdit::singleline(&mut example.title)
                            .id(Id::new("title_edit"))
                            .hint_text("给这条示例起个名字")
                            .desired_width(240.0),
                    );
                    if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        self.save_current();
                    }
                } else {
                    ui.add_enabled(false, TextEdit::singleline(&mut String::new()));
                }

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

                ui.separator();
                pick_image = ui
                    .button("选择图片…")
                    .on_hover_text("图片会复制进图库托管的 images/ 目录；也可以直接把图片拖进窗口")
                    .clicked();
                if self
                    .current
                    .as_ref()
                    .map(|example| example.image_hash.is_some())
                    .unwrap_or(false)
                {
                    clear_image = ui
                        .button("移除图片")
                        .on_hover_text("只解除引用，托管目录里的图片文件不会被删除")
                        .clicked();
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    open_data_dir = ui.button("打开数据目录").clicked();
                    import = ui.button("导入图库…").clicked();
                    open_export = ui.button("导出图库…").clicked();
                });
            });
            ui.add_space(6.0);

            if pick_image {
                let path = rfd::FileDialog::new()
                    .add_filter("图片", &IMAGE_EXTENSIONS)
                    .pick_file();
                if let Some(path) = path {
                    self.embed_image_file(ui.ctx(), &path);
                }
            }
            if clear_image {
                if let Some(example) = self.current.as_mut() {
                    example.image_hash = None;
                    example.image_ext = None;
                }
                self.cache.drop_preview();
                self.set_status(ui.ctx(), "已移除图片引用".to_string());
            }
            if open_data_dir {
                open_in_explorer(Path::new(&self.data_dir));
            }
            if import {
                self.import_dialog(ui.ctx());
            }
            if open_export {
                self.export_path = Path::new(&self.data_dir)
                    .join(archive::default_file_name(now_unix()))
                    .to_string_lossy()
                    .to_string();
                self.export_open = true;
            }
        });
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

                // 快照一次：遍历期间可能触发选中/新建，避免与 `store` 的可变借用冲突。
                let rows: Vec<(i64, String, Option<String>)> = self
                    .store
                    .rows()
                    .iter()
                    .map(|row| (row.id, row.display_title().to_string(), row.image_file_name()))
                    .collect();
                let current_id = self.current_id();
                let images_dir = self.store.images_dir();
                let row_count = rows.len();

                let mut clicked: Option<Option<i64>> = None;
                let mut insert_at: Option<(i64, usize)> = None;
                let mut drag_stopped = false;

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (index, (id, title, image_file)) in rows.iter().enumerate() {
                            let selected = Some(*id) == current_id;
                            let width = (ui.available_width() - 8.0).max(60.0);
                            let (rect, response) = ui
                                .allocate_exact_size(Vec2::new(width, 52.0), Sense::click_and_drag());

                            // 长按判定：按住超过 REORDER_HOLD 且产生位移，才进入重排。
                            if response.drag_started() {
                                self.drag_row_id = Some(*id);
                                self.drag_started = Instant::now();
                                self.drag_moved = false;
                                self.drag_target = None;
                            }
                            if response.dragged() {
                                if response.drag_delta().length() > REORDER_SLOP {
                                    self.drag_moved = true;
                                }
                                let held = Instant::now().duration_since(self.drag_started);
                                if held >= REORDER_HOLD && self.drag_moved {
                                    let before = ui
                                        .ctx()
                                        .pointer_hover_pos()
                                        .map(|pos| {
                                            if pos.y >= rect.center().y {
                                                index + 1
                                            } else {
                                                index
                                            }
                                        })
                                        .unwrap_or(index);
                                    self.drag_target = Some(before.min(row_count));
                                }
                            }
                            if response.drag_stopped() {
                                drag_stopped = true;
                            }
                            if response.clicked() && !self.drag_moved {
                                clicked = Some(Some(*id));
                            }

                            // 长按拖拽时画一条插入位置指示线。
                            if self.drag_row_id == Some(*id) && self.drag_moved {
                                if let Some(insert) = self.drag_target {
                                    let y = if insert > index {
                                        rect.bottom() + 1.0
                                    } else {
                                        rect.top() - 1.0
                                    };
                                    ui.painter().hline(
                                        rect.x_range(),
                                        y,
                                        Stroke::new(2.0, Color32::from_rgb(120, 170, 255)),
                                    );
                                }
                            }

                            paint_sidebar_row(
                                ui,
                                rect,
                                selected,
                                &response,
                                title,
                                image_file.as_deref(),
                                &images_dir,
                                &mut self.cache,
                            );
                        }
                        ui.add_space(4.0);
                    });

                // 指针落到列表下方 => 插到末尾。
                if self.drag_row_id.is_some() && self.drag_moved {
                    if let Some(pos) = ui.ctx().pointer_hover_pos() {
                        let bottom = ui.min_rect().bottom();
                        if pos.y > bottom {
                            self.drag_target = Some(row_count);
                        }
                    }
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

    /// 长按拖拽重排：把 `from` 移到 `insert` 位置并落库。
    fn move_row(&mut self, from: i64, insert: usize) {
        let mut order: Vec<i64> = self.store.rows().iter().map(|row| row.id).collect();
        let Some(source) = order.iter().position(|id| *id == from) else {
            return;
        };
        let mut destination = insert.min(order.len());
        if destination > source {
            destination -= 1;
        }
        if destination == source {
            return;
        }
        let moved = order.remove(source);
        order.insert(destination, moved);
        if let Err(err) = self.store.reorder(&order) {
            self.set_error(err);
        }
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        let actions = egui::Panel::bottom("bottom_bar")
            .show(ui, |ui| {
                ui.add_space(4.0);
                let mut actions: Vec<Action> = Vec::new();
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
                        .add_enabled(self.current_id().is_some(), egui::Button::new("重命名"))
                        .on_hover_text("Ctrl+R")
                        .clicked()
                    {
                        actions.push(Action::Rename);
                    }
                    if ui
                        .add_enabled(self.current_id().is_some(), egui::Button::new("删除"))
                        .clicked()
                    {
                        actions.push(Action::Delete);
                    }

                    ui.separator();
                    let status = self.status_message(ui.ctx());
                    ui.label(RichText::new(status).color(HEADER_GRAY).size(12.0));
                });
                ui.add_space(4.0);
                actions
            })
            .inner;

        for action in actions {
            match action {
                Action::Save => self.save_current(),
                Action::SaveAs => self.save_as(),
                Action::Rename => {
                    if let Some(example) = &self.current {
                        self.rename_buffer = example.title.clone();
                        self.rename_open = true;
                    }
                }
                Action::Delete => self.delete_confirm = true,
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
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), IMAGE_BOX_FILL);

        let image_key = self
            .current
            .as_ref()
            .and_then(|example| example.image_file_name());
        let path = self
            .current
            .as_ref()
            .and_then(|example| example.image_path(&self.store.images_dir()));

        match (image_key, path) {
            (Some(key), Some(path)) => match self.cache.preview(ui.ctx(), &key, &path) {
                Ok(cached) => {
                    let size = fit_size(cached.aspect(), rect.size() - Vec2::splat(8.0));
                    let target = Rect::from_center_size(rect.center(), size);
                    egui::Image::new(&cached.texture)
                        .fit_to_exact_size(size)
                        .paint_at(ui, target);
                    if let Some(hash) = key.split('.').next() {
                        let short = &hash[..hash.len().min(16)];
                        ui.interact(target, Id::new("image_preview"), Sense::hover())
                            .on_hover_text(format!("内容哈希：{short}…\n{key}"));
                    }
                }
                Err(err) => {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("图片无法显示\n{err}"),
                        egui::FontId::proportional(13.0),
                        Color32::from_gray(190),
                    );
                }
            },
            _ => {
                let pending = match self.pending_image_size {
                    Some((w, h)) => format!("已选择图片 {w}×{h}（保存后入库）\n"),
                    None => String::new(),
                };
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("{pending}把图片拖到这里，或点上方「选择图片…」"),
                    egui::FontId::proportional(13.0),
                    Color32::from_gray(140),
                );
            }
        }

        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(6),
            Stroke::new(1.0, Color32::from_gray(70)),
            StrokeKind::Inside,
        );
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

        // 重命名。
        if self.rename_open {
            let mut close = false;
            let mut confirm = false;
            egui::Modal::new(Id::new("rename_modal")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.heading("重命名示例");
                ui.add_space(6.0);
                let response = ui.add(
                    TextEdit::singleline(&mut self.rename_buffer)
                        .hint_text("示例标题")
                        .desired_width(f32::INFINITY),
                );
                response.request_focus();
                if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    confirm = true;
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("确定").clicked() {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                });
            });

            if confirm {
                let title = self.rename_buffer.trim().to_string();
                match self.current_id() {
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
                                    self.set_status(ctx, "已重命名".to_string());
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
                close = true;
            }
            if close {
                self.rename_open = false;
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

/// 底部按钮触发的动作，由闭包收集后在外层统一执行（避免与 `self` 的借用纠缠）。
#[derive(Clone, Copy)]
enum Action {
    Save,
    SaveAs,
    Rename,
    Delete,
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

/// 画一整行侧边栏条目：缩略图 + 标题，选中态高亮。
fn paint_sidebar_row(
    ui: &mut egui::Ui,
    rect: Rect,
    selected: bool,
    response: &egui::Response,
    title: &str,
    image_file: Option<&str>,
    images_dir: &Path,
    cache: &mut ImageCache,
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
    let thumb_rect = Rect::from_min_size(
        rect.min + Vec2::new(6.0, 6.0),
        Vec2::splat(rect.height() - 12.0),
    );
    let mut drew_thumb = false;
    if let Some(file_name) = image_file {
        if let Some(key) = file_name.split('.').next() {
            let path = images_dir.join(file_name);
            if let Ok(cached) = cache.thumbnail(ui.ctx(), key, &path) {
                let size = fit_size(cached.aspect(), thumb_rect.size());
                let target = Rect::from_center_size(thumb_rect.center(), size);
                egui::Image::new(&cached.texture)
                    .fit_to_exact_size(size)
                    .paint_at(ui, target);
                drew_thumb = true;
            }
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

    // 标题：用 painter 直接画，缩略图与标题因此可以在同一行精确对齐。
    let text_left = thumb_rect.right() + 8.0;
    let text_rect = Rect::from_min_max(
        egui::pos2(text_left, rect.top() + 6.0),
        egui::pos2(rect.right() - 6.0, rect.bottom() - 6.0),
    );
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
        egui::pos2(text_left, text_rect.top()),
        galley,
        Color32::PLACEHOLDER,
    );
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
