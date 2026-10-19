//! Prompt Gallery —— 本地提示词图库。
//!
//! 需求与决策来源：
//! * `CONTEXT.md`：术语表；窗口标题、导出文件名前缀等用户可见文案。
//! * `docs/adr/0001-egui-eframe-for-low-memory-ui.md`：egui/eframe，隐藏滚动条、
//!   长按拖拽重排、不可选中灰字都要自己画。
//! * `docs/adr/0011-prompt-flattened-to-positive-and-negative.md`：正/负向提示词各一个
//!   独立文本框，不再分区。
//! * `docs/adr/0003-sqlite-metadata-plus-hash-named-image-files.md`：SQLite + 内容哈希图片文件。
//!
//! 依赖版本注记：本项目使用 egui/eframe 0.36。该版本的 `eframe::App` 用 `ui(&mut Ui, &mut Frame)`
//! 而不是 `update(&Context, …)`，并且面板统一为 `egui::Panel::{left,right,top,bottom}`
//! （`show(ui, …)` 接收 `&mut Ui`）。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use eframe::egui;
use egui::{
    Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Id, Key, Layout,
    Modifiers, Rect, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit, Vec2,
};

use prompt_gallery::archive;
use prompt_gallery::comfy::{self, Detection};
use prompt_gallery::images::{fit_size, ImageCache};
use prompt_gallery::model::{now_unix, Example, PromptSource};
use prompt_gallery::store::{self, Store};

mod icon;
mod testhook;
mod theme;

/// 窗口标题；同时是应用显示名（CONTEXT.md）。
const APP_TITLE: &str = "Prompt Gallery";
/// 侧边栏默认宽度。
const SIDEBAR_WIDTH: f32 = 248.0;
/// 自绘标题栏高度（ADR-0009）。
const TITLE_BAR_HEIGHT: f32 = 40.0;
/// 标题栏右侧窗口按钮宽度。
const WINDOW_BUTTON_WIDTH: f32 = 46.0;
/// 窗口八向缩放热区厚度。
const RESIZE_EDGE: f32 = 5.0;
/// 窗口四角缩放热区边长。
const RESIZE_CORNER: f32 = 12.0;
/// 「长按拖动」达标时间：从指针**物理按下**时刻计时（§1，ADR-0005）。
const REORDER_HOLD: Duration = Duration::from_millis(280);
/// 已落库示例的自动保存防抖窗口（ADR-0006）。
const AUTOSAVE_DEBOUNCE: Duration = Duration::from_millis(500);
/// 达标前允许的最大累计位移；超过即取消本次长按（§1：6pt，ADR-0005）。
const REORDER_CANCEL_SLOP: f32 = 6.0;
/// 达标后产生位移、进入「重排中」的阈值（§1：4pt）。
const REORDER_SLOP: f32 = 4.0;
/// 拖到列表上 / 下边缘时的自动滚动热区高度（§3：20pt）。
const AUTOSCROLL_ZONE: f32 = 20.0;
/// 自动滚动热区内缘速度（§3：约 4pt/帧 ≈ 240pt/s）。
const AUTOSCROLL_MIN_SPEED: f32 = 240.0;
/// 自动滚动最大速度（§3：贴到视口边缘约 16pt/帧 ≈ 960pt/s）。
const AUTOSCROLL_MAX_SPEED: f32 = 960.0;
/// 虚影达标时的上浮量（§2：2–4pt）。
const DRAG_GHOST_UPLIFT: f32 = 4.0;
/// 被拖源行原位淡占位的不透明度（§2：约 35%）。
const DRAG_SOURCE_OPACITY: f32 = 0.35;
/// 虚影不透明度（§2：约 85%）。
const DRAG_GHOST_OPACITY: f32 = 0.85;
/// 状态文案自动淡出的秒数。
const STATUS_TTL: f64 = 12.0;

/// 中央区两块之间的缝宽（pt）。
const CENTRAL_GAP: f32 = 6.0;
/// 图片框在长边方向的占比：横版取可用高、竖版取可用宽（同一常数，观感一致）。
const IMAGE_SHARE: f32 = 0.42;
/// 竖版文本列内正向提示词占比，其余给负向。
const POSITIVE_SHARE: f32 = 0.62;
/// 竖版安全网：图片列最小宽（pt）。
const MIN_IMAGE_COL_W: f32 = 240.0;
/// 竖版安全网：文本列最小宽（pt），不足则退回横版。
const MIN_TEXT_COL_W: f32 = 320.0;
/// 文本框右上角复制按钮的尺寸与内边距（pt）。
const COPY_BTN_W: f32 = 44.0;
const COPY_BTN_H: f32 = 18.0;
/// 复制按钮反馈文案的停留时长（秒）。
const COPY_FLASH: f64 = 0.8;

/// 支持的图片后缀（对话框过滤用）。
const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_TITLE)
            .with_app_id("prompt-gallery")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([880.0, 560.0])
            // ADR-0009：不要系统标题栏，自己画；透明让圆角外露背景。
            .with_decorations(false)
            .with_transparent(true)
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
                Ok(store) => {
                    // 外观设置与数据库同目录：先读配置，再决定首帧皮肤。
                    let config = theme::load_config(store.data_dir());
                    cc.egui_ctx.set_theme(config.theme.preference());
                    theme::apply(&cc.egui_ctx);
                    Ok(Box::new(GalleryView::new(store, config.theme)))
                }
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
        theme::apply(ui.ctx());
        let p = theme::Palette::current(ui.ctx());
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            ui.label(
                RichText::new(format!("{APP_TITLE} 无法启动"))
                    .size(20.0)
                    .strong()
                    .color(p.text_strong),
            );
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
    /// 外观设置（跟随系统 / 浅色 / 深色），持久化在 config.json。
    theme_choice: theme::ThemeChoice,
    /// 上一帧实际生效的皮肤；与 ctx.theme() 不同就重挂 Style。
    applied_theme: Option<egui::Theme>,
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
    /// 导出弹窗里被勾选的示例 id（默认全选，§4）。
    export_selected: BTreeSet<i64>,
    /// 侧边栏长按拖动的手势状态（§1，替代旧的
    /// `drag_row_id` / `drag_moved` / `drag_started` / `drag_target`）。
    row_drag: Option<RowDrag>,
    /// 自动保存到点时刻（egui 时间，秒）；None 表示没有待写改动（ADR-0006）。
    autosave_deadline: Option<f64>,
    /// 关窗确认弹窗是否打开（仅非空草稿会用到，ADR-0006 D9）。
    close_confirm: bool,
    /// 用户已在关窗确认里选择放弃草稿，允许真正关闭。
    close_confirmed: bool,
    /// 待确认的「图片内提示词会清空现有文本」选择；字节已读入内存，确认前后看到的是同一张图。
    prompt_confirm: Option<PendingChoice>,
    /// 复制按钮的反馈：哪一侧、何时消隐（`副本已复制` 文案只停 0.8s）。
    copy_flash: Option<(PromptKind, f64)>,
    /// 应用内测试钩子（ADR-0008）：仅当环境变量启用且是 debug 构建时为 Some。
    hook: Option<testhook::TestHook>,
}

impl GalleryView {
    fn new(store: Store, theme_choice: theme::ThemeChoice) -> Self {
        let data_dir = store.data_dir().to_string_lossy().to_string();
        let mut view = Self {
            store,
            current: None,
            baseline: None,
            data_dir,
            theme_choice,
            applied_theme: None,
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
            export_selected: BTreeSet::new(),
            row_drag: None,
            autosave_deadline: None,
            close_confirm: false,
            close_confirmed: false,
            prompt_confirm: None,
            copy_flash: None,
            hook: testhook::TestHook::from_env(),
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

    /// 当前示例是否被「来源锁」锁住（提示词由程序从图片读出，解不开，ADR-0012）。
    ///
    /// 来源锁只锁内容（提示词两段、图片与锁本身），不锁标题与删除：
    /// 否则 ComfyUI 示例一落库就既改不了名、也删不掉。
    fn current_source_locked(&self) -> bool {
        self.current
            .as_ref()
            .map_or(false, |example| example.prompt_source.is_locked_source())
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
        if self.current_id().is_some() {
            // 已落库示例：切换前先把自动保存落库，然后直接切，不弹任何框（ADR-0006）。
            if !self.flush_now() {
                return;
            }
            match id {
                Some(id) => self.load_example(id),
                None => self.create_draft(),
            }
            return;
        }
        // 草稿：只有非空草稿才需要确认（ADR-0006 D9）。
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

    /// 立即把当前已落库示例的未写改动落库（自动保存，ADR-0006）。
    ///
    /// 成功返回 true；失败时保留内存内容、把错误写进状态栏并返回 false。
    /// 草稿不在此列——草稿仍然「点保存才落库」。
    fn flush_now(&mut self) -> bool {
        self.autosave_deadline = None;
        if self.current_id().is_none() {
            return true;
        }
        let locked = self.current.as_ref().map(|e| e.locked).unwrap_or(false);
        if locked || !self.dirty() {
            return true;
        }
        let store = &mut self.store;
        let mut result: anyhow::Result<()> = Ok(());
        if let Some(example) = self.current.as_mut() {
            example.title = example.title.trim().to_string();
            result = store.update(example);
        }
        match result {
            Ok(()) => {
                self.baseline = self.current.clone();
                true
            }
            Err(err) => {
                self.set_error(err);
                false
            }
        }
    }

    /// 关窗前是否需要草稿确认：只有非空草稿才算（ADR-0006 D9）。
    fn draft_protection_needed(&self) -> bool {
        self.current_id().is_none() && self.dirty()
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

    /// 选图/拖图：读一次源文件做校验，检测 ComfyUI 提示词，再决定是否直接换图。
    ///
    /// ADR-0004：草稿仍只记源路径，保存时才按内容哈希入库；已落库示例选图即刻入库（ADR-0006 D3）。
    /// ADR-0013：图片自带提示词且当前已有文本时，先挂起确认（图片也先不换，确认前后一致），
    /// 确认后才「清空两段 → 写入 → 上来源锁」一次性完成。
    fn set_pending_image(&mut self, ctx: &egui::Context, path: &Path) {
        if self.current.as_ref().map_or(false, |e| e.locked) {
            self.set_status(ctx, "示例已锁定，无法更换图片".to_string());
            return;
        }
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) => {
                let err = anyhow::Error::new(err)
                    .context(format!("读取图片失败：{}", path.display()));
                self.set_error(err);
                return;
            }
        };
        let Some(ext) = store::ext_for_bytes(&bytes) else {
            self.set_status(ctx, format!("无法识别的图片格式：{}", path.display()));
            return;
        };
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            self.set_status(
                ctx,
                format!("图片过大（{} MB），已拒绝载入", bytes.len() / 1024 / 1024),
            );
            return;
        }
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let size = image_size_of(&bytes);
        let detection = comfy::detect(&bytes);
        let choice = PendingChoice {
            path: path.to_path_buf(),
            label,
            key: format!("pending:{}", path.display()),
            bytes,
            ext,
            size,
            detection,
        };
        let has_text = self.current.as_ref().map_or(false, |e| !e.prompts_empty());
        if has_text && matches!(choice.detection, Detection::Found { .. }) {
            self.prompt_confirm = Some(choice);
            self.set_status(ctx, "检测到 ComfyUI 提示词：确认后会清空当前文本".to_string());
            return;
        }
        self.commit_image_choice(ctx, choice);
    }

    /// 把一次选图结果落进当前示例：换图，必要时写入图片提示词并上来源锁。
    fn commit_image_choice(&mut self, ctx: &egui::Context, choice: PendingChoice) {
        let size_hint = match choice.size {
            Some((w, h)) => format!(" {w}×{h}"),
            None => String::new(),
        };
        let extracted = match &choice.detection {
            Detection::Found { positive, negative } => Some((positive.clone(), negative.clone())),
            _ => None,
        };
        let pending = self.current.as_ref().map_or(true, |e| e.is_draft());

        if let Some(id) = self.current_id() {
            let (hash, ext) = match self.store.store_image_bytes(&choice.bytes, choice.ext) {
                Ok(pair) => pair,
                Err(err) => {
                    self.set_error(err);
                    return;
                }
            };
            match extracted {
                // 有图片内提示词：换图 + 清空并写入两段 + 上来源锁，一条 UPDATE 原子完成（ADR-0012）。
                Some((positive, negative)) => {
                    if let Err(err) = self.store.apply_image_prompt(id, &hash, &ext, &positive, &negative)
                    {
                        self.set_error(err);
                        return;
                    }
                    self.cache.drop_preview();
                    self.load_example(id);
                    self.set_status(
                        ctx,
                        format!("已从 ComfyUI 图片读取提示词，已锁定{size_hint}"),
                    );
                }
                None => {
                    if let Some(example) = self.current.as_mut() {
                        example.image_hash = Some(hash);
                        example.image_ext = Some(ext);
                    }
                    self.cache.drop_preview();
                    self.flush_now();
                    self.set_status(ctx, selection_status(&choice.detection, &size_hint, false));
                }
            }
            return;
        }

        // 草稿：仍然只记源路径，保存时才入库（ADR-0004）。
        self.pending_image = Some(PendingImage {
            path: choice.path,
            label: choice.label,
            key: choice.key,
        });
        self.cache.drop_preview();
        match extracted {
            // 草稿期也立刻上锁（ADR-0012）：否则保存后这条文本的来源就说不清了。
            Some((positive, negative)) => {
                if let Some(example) = self.current.as_mut() {
                    example.positive = positive;
                    example.negative = negative;
                    example.prompt_source = PromptSource::Image;
                    example.locked = true;
                }
                self.set_status(
                    ctx,
                    format!("已从 ComfyUI 图片读取提示词，已锁定{size_hint}（保存后入库）"),
                );
            }
            None => self.set_status(
                ctx,
                selection_status(&choice.detection, &size_hint, pending),
            ),
        }
    }

    fn export_to(&mut self, ctx: &egui::Context) {
        // 导出前先把自动保存落库（ADR-0006）。
        if !self.flush_now() {
            return;
        }
        let mut target = PathBuf::from(self.export_path.trim());
        if target.as_os_str().is_empty() {
            self.set_error(anyhow::anyhow!("导出路径为空"));
            return;
        }
        if target.extension().is_none() {
            target.set_extension("zip");
        }
        // 只导出勾选的示例，顺序与侧边栏一致（D7）。
        let ids: Vec<i64> = self
            .store
            .rows()
            .iter()
            .map(|row| row.id)
            .filter(|id| self.export_selected.contains(id))
            .collect();
        match archive::export_ids(&self.store, &ids, &target) {
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
/// 而不是静默丢图；内容哈希命名让同一张图在多次入库/多条示例间复用同一个文件。
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

/// 确认弹窗里的提示词预览：取前 `limit` 个字符，超出补省略号。
fn preview_excerpt(text: &str, limit: usize) -> String {
    let mut excerpt: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        excerpt.push('…');
    }
    excerpt
}

/// 应用测试钩子递进来的「选了一张图」（ADR-0008）：与拖拽 / 点击选图同一条路径。
///
/// 写成自由函数而不是方法：钩子只在 debug 构建里有真实现，这里保持与 `set_pending_image`
/// 同一条路径即可，不需要为它动 `impl GalleryView` 的结构。
fn pump_hook_image(view: &mut GalleryView, ctx: &egui::Context) {
    let Some(path) = view
        .hook
        .as_mut()
        .and_then(|hook| hook.take_image_request())
        .map(|(_seq, path)| path)
    else {
        return;
    };
    view.set_pending_image(ctx, std::path::Path::new(&path));
    if let Some(hook) = view.hook.as_mut() {
        hook.finish_image_request(Ok(String::new()));
    }
}

/// 选图后的状态文案（ADR-0013 的三态）；`pending` 为真时补「保存后入库」。
fn selection_status(detection: &Detection, size_hint: &str, pending: bool) -> String {
    let suffix = if pending { "（保存后入库）" } else { "" };
    match detection {
        Detection::NoPositive => format!("检测到 ComfyUI 图片，但未找到提示词{suffix}"),
        _ => format!("已选择图片{size_hint}{suffix}"),
    }
}

// ---------------------------------------------------------------- 界面

impl eframe::App for GalleryView {
    /// 应用内测试钩子（ADR-0008）：在 egui 处理本帧之前注入合成输入、收发指令。
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(hook) = self.hook.as_mut() {
            hook.tick(ctx, raw_input);
        }
    }

    /// 透明窗口：圆角之外必须是真透明，否则会露出黑色方角（ADR-0009）。
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // 系统深浅色可能在运行中变化：只在真的变了时重挂一次 Style。
        let current_theme = ctx.theme();
        if self.applied_theme != Some(current_theme) {
            self.applied_theme = Some(current_theme);
            theme::apply(&ctx);
        }

        // 拖进窗口的图片文件（与侧边栏重排不冲突：那是内部拖拽，这里是外部文件）。
        // ADR-0004：只记住源路径，保存时才按内容哈希入库。
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(file) = dropped.into_iter().next() {
            let path = file.path().to_path_buf();
            self.set_pending_image(&ctx, &path);
        }
        // 测试钩子的「选图」指令走同一条路径（原生文件对话框没法被合成输入驱动）。
        pump_hook_image(self, &ctx);

        // 自动保存到点：把已落库示例的改动写库（ADR-0006）。
        if let Some(deadline) = self.autosave_deadline {
            if ctx.input(|i| i.time) >= deadline {
                self.flush_now();
            }
        }

        // 关窗拦截（ADR-0006 D9）：已落库示例先落库；非空草稿先确认。
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.close_confirmed {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if self.draft_protection_needed() {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.close_confirm = true;
            } else if self.flush_now() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
        }

        self.handle_shortcuts(&ctx);

        // 自绘窗口：圆角画布 + 顶部标题栏，其余面板照旧（ADR-0009）。
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        theme::window_frame(&ctx, maximized).show(ui, |ui| {
            self.title_bar(ui);
            self.sidebar(ui);
            self.bottom_bar(ui);
            self.central(ui);
        });
        self.dialogs(&ctx);
        self.tick_status(&ctx);
        self.resize_grips(&ctx, maximized);

        // 拖动中每帧重绘：虚影跟手、热区自动滚动、落点实时重算（§1、§2、§3）。
        if self.editing_title.is_some()
            || self.delete_confirm
            || self.pending_select.is_some()
            || self.about_open
            || self.row_drag.is_some()
            || self.close_confirm
            || self.prompt_confirm.is_some()
            || self.copy_flash.is_some()
        {
            ctx.request_repaint();
        }
    }
}

impl GalleryView {
    // ------------------------------------------------------------ 标题栏（ADR-0009）

    fn title_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let p = theme::Palette::current(&ctx);
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));

        egui::Panel::top("title_bar")
            .exact_size(TITLE_BAR_HEIGHT)
            .frame(egui::Frame::new())
            .show(ui, |ui| {
                let rect = ui.max_rect();
                let radius = if maximized {
                    CornerRadius::ZERO
                } else {
                    // 比窗口外框（10）小 1pt：正好落在 1pt 描边内侧。
                    CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 }
                };
                ui.painter().rect_filled(rect, radius, p.surface_raised);

                // 拖动热区先登记：后画的按钮与菜单在上层，点击不会被拖动吃掉。
                let drag = ui.interact(rect, Id::new("title_bar_drag"), Sense::click_and_drag());
                let mut on_child = false;

                // 左右两组都嵌在这一个闭包内：平级各开一个 scope 时，第二组会被换行挤到下一行并裁掉。
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add_space(12.0);
                        let (icon_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                        icon::paint(ui.painter(), icon_rect);
                        ui.add_space(2.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(APP_TITLE).size(14.0).color(p.text_strong),
                            )
                            .selectable(false),
                        );
                        ui.add_space(8.0);

                        // 「数据管理」是导出 / 导入 / 打开数据目录的唯一入口（D5）。
                        let data = chrome_button(ui, "数据管理", &p);
                        on_child |= data.hovered();
                        egui::Popup::menu(&data).show(|ui| {
                            ui.set_min_width(180.0);
                            let mut action = None;
                            if menu_item(ui, "导出图库", false, &p).clicked() {
                                action = Some(Action::Export);
                            }
                            if menu_item(ui, "导入图库", false, &p).clicked() {
                                action = Some(Action::Import);
                            }
                            if menu_item(ui, "打开数据目录", false, &p).clicked() {
                                action = Some(Action::OpenDataDir);
                            }
                            if let Some(action) = action {
                                self.run_action(&ctx, action);
                            }
                        });

                        let about = chrome_button(ui, "关于", &p);
                        on_child |= about.hovered();
                        if about.clicked() {
                            self.about_open = true;
                        }

                        let appearance = chrome_button(ui, "外观", &p);
                        on_child |= appearance.hovered();
                        egui::Popup::menu(&appearance).show(|ui| {
                            ui.set_min_width(180.0);
                            let mut chosen = None;
                            for (choice, label) in [
                                (theme::ThemeChoice::System, "跟随系统"),
                                (theme::ThemeChoice::Light, "浅色"),
                                (theme::ThemeChoice::Dark, "深色"),
                            ] {
                                let selected = self.theme_choice == choice;
                                if menu_item(ui, label, selected, &p).clicked() {
                                    chosen = Some(choice);
                                }
                            }
                            if let Some(choice) = chosen {
                                self.set_theme_choice(&ctx, choice);
                            }
                        });
                        // 右侧窗口按钮：同一行内从右往左排，贴边零间距。
                        ui.scope(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let close = window_button(ui, WindowButton::Close, &p);
                                on_child |= close.hovered();
                                if close.clicked() {
                                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                                let maximize = window_button(ui, WindowButton::Maximize, &p);
                                on_child |= maximize.hovered();
                                if maximize.clicked() {
                                    ui.ctx()
                                        .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                                }
                                let minimize = window_button(ui, WindowButton::Minimize, &p);
                                on_child |= minimize.hovered();
                                if minimize.clicked() {
                                    ui.ctx()
                                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                                }
                            });
                        });
                    });
                });
                // 拖动整条标题栏；双击最大化 / 还原。
                if drag.drag_started_by(egui::PointerButton::Primary) && !on_child {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if drag.double_clicked() && !on_child {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                }
            });
    }

    /// 兼容顶栏与底栏的同一个动作分发点。
    fn run_action(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Save => self.save_current(),
            Action::Delete => self.delete_confirm = true,
            Action::Export => {
                self.export_path = Path::new(&self.data_dir)
                    .join(archive::default_file_name(now_unix()))
                    .to_string_lossy()
                    .to_string();
                // 每次打开都默认全选（D7）。
                self.export_selected = self.store.rows().iter().map(|row| row.id).collect();
                self.export_open = true;
            }
            Action::Import => self.import_dialog(ctx),
            Action::OpenDataDir => open_in_explorer(Path::new(&self.data_dir)),
        }
    }

    fn set_theme_choice(&mut self, ctx: &egui::Context, choice: theme::ThemeChoice) {
        self.theme_choice = choice;
        ctx.set_theme(choice.preference());
        theme::apply(ctx);
        self.applied_theme = Some(ctx.theme());
        let config = theme::AppConfig { theme: choice };
        if theme::save_config(Path::new(&self.data_dir), &config).is_err() {
            self.set_status(ctx, "主题设置未能保存".to_string());
        }
    }

    /// 八向缩放手势热区：系统标题栏没了，缩放要自己接（ADR-0009）。
    fn resize_grips(&self, ctx: &egui::Context, maximized: bool) {
        if maximized {
            return;
        }
        // 用局部坐标（原点在客户区左上）；viewport().inner_rect 是屏幕坐标，
        // 直接拿来会把热区整体平移出窗口。
        let screen = ctx.content_rect();
        let (left, top) = (screen.min.x, screen.min.y);
        let (right, bottom) = (screen.max.x, screen.max.y);
        let e = RESIZE_EDGE;
        let c = RESIZE_CORNER;
        let grips = [
            (
                "north",
                Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, top + e)),
                egui::ResizeDirection::North,
                CursorIcon::ResizeNorth,
            ),
            (
                "south",
                Rect::from_min_max(egui::pos2(left, bottom - e), egui::pos2(right, bottom)),
                egui::ResizeDirection::South,
                CursorIcon::ResizeSouth,
            ),
            (
                "west",
                Rect::from_min_max(egui::pos2(left, top), egui::pos2(left + e, bottom)),
                egui::ResizeDirection::West,
                CursorIcon::ResizeWest,
            ),
            (
                "east",
                Rect::from_min_max(egui::pos2(right - e, top), egui::pos2(right, bottom)),
                egui::ResizeDirection::East,
                CursorIcon::ResizeEast,
            ),
            (
                "nw",
                Rect::from_min_max(egui::pos2(left, top), egui::pos2(left + c, top + c)),
                egui::ResizeDirection::NorthWest,
                CursorIcon::ResizeNorthWest,
            ),
            (
                "ne",
                Rect::from_min_max(
                    egui::pos2(right - c, top),
                    egui::pos2(right, top + c),
                ),
                egui::ResizeDirection::NorthEast,
                CursorIcon::ResizeNorthEast,
            ),
            (
                "sw",
                Rect::from_min_max(
                    egui::pos2(left, bottom - c),
                    egui::pos2(left + c, bottom),
                ),
                egui::ResizeDirection::SouthWest,
                CursorIcon::ResizeSouthWest,
            ),
            (
                "se",
                Rect::from_min_max(
                    egui::pos2(right - c, bottom - c),
                    egui::pos2(right, bottom),
                ),
                egui::ResizeDirection::SouthEast,
                CursorIcon::ResizeSouthEast,
            ),
        ];
        for (name, rect, direction, cursor) in grips {
            egui::Area::new(Id::new(("resize_grip", name)))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.min)
                .movable(false)
                .interactable(true)
                .show(ctx, |ui| {
                    let (_, response) = ui.allocate_exact_size(rect.size(), Sense::drag());
                    let response = response.on_hover_cursor(cursor);
                    if response.drag_started() {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
                    }
                });
        }
    }

    /// 状态时间戳的延迟刷新：set_status 拿不到 ctx 时先写 NaN，这里补上。
    fn tick_status(&mut self, ctx: &egui::Context) {
        if self.status_time.is_nan() {
            self.status_time = ctx.input(|i| i.time);
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (save, rename) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::CTRL, Key::S),
                i.consume_key(Modifiers::CTRL, Key::R),
            )
        });
        if save {
            self.save_current();
        }
        if rename {
            // §2：Ctrl+R 改为当前行的侧边栏内联编辑，不再弹重命名窗。
            // 手动锁定示例禁止改名（ADR-0007）；来源锁不限制改名（ADR-0012）。
            let locked = self.current.as_ref().map(|e| e.locked).unwrap_or(false);
            if !locked || self.current_source_locked() {
                let target = self.current_id();
                self.begin_inline_edit(target);
            }
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
                // §4：计数块（「示例 N」+ separator）整块删除，列表从面板顶部开始，只留 3pt 内边距。
                ui.add_space(3.0);
                // 固定的「+新示例」行：始终可见、不可拖动、不进落点计算、永不高亮为选中蓝。
                if new_example_row(ui) {
                    self.select(None);
                }
                ui.add_space(3.0);
                // 固定行与可滚动列表之间的 1pt 分隔线（§4：区分「固定」与「滚动」）。
                let (seam_rect, _) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
                ui.painter().hline(
                    seam_rect.x_range(),
                    seam_rect.center().y,
                    Stroke::new(1.0, theme::Palette::current(ui.ctx()).border),
                );
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
                        locked: false,
                        source_locked: false,
                    });
                }
                for row in self.store.rows() {
                    rows.push(SidebarRow {
                        id: Some(row.id),
                        title: row.display_title().to_string(),
                        thumb: row.image_file_name().map(ThumbSource::Managed),
                        locked: row.locked,
                        source_locked: row.prompt_source.is_locked_source(),
                    });
                }

                let mut clicked: Option<Option<i64>> = None;
                let mut edit_start: Option<Option<i64>> = None;
                let mut edit_result: Option<InlineEditResult> = None;
                let mut insert_at: Option<(i64, usize)> = None;
                // 点击锁图标的结果，待列表渲染完后再写库（ADR-0007）。
                let mut lock_toggle: Option<(i64, bool)> = None;
                // 来源锁点了也不改状态：沿用 ADR-0012，只提示一句。
                let mut unlock_blocked = false;
                // 收集每一行的矩形：落点要相对所有行判断（草稿行不参与重排，不进 row_rects）。
                let mut row_rects: Vec<Rect> = Vec::with_capacity(rows.len());
                // 本帧指针物理按下所在的行：长按手势的起点（§1，ADR-0005）。
                let mut down_on: Option<(i64, Rect)> = None;
                // 被拖源行的本帧矩形：淡占位与「松手是否落在标题上」判定共用。
                let mut source_rect: Option<Rect> = None;
                // 松手结果，待 ScrollArea 渲染完后再派发（避免与 self 的借用纠缠）。
                let mut release: Option<(i64, GestureOutcome, Option<usize>)> = None;
                let ctx = ui.ctx().clone();

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        // 滚动视口：热区与越界落点判定都要用它（§3）。
                        let viewport = ui.clip_rect();
                        let (
                            pointer_down,
                            released,
                            pressed_now,
                            press_start,
                            press_origin,
                            drag_delta,
                            now,
                            dt,
                        ) = ctx.input(|i| {
                            (
                                i.pointer.primary_down(),
                                i.pointer.primary_released(),
                                i.pointer.primary_pressed(),
                                i.pointer.press_start_time(),
                                i.pointer.press_origin(),
                                i.pointer.total_drag_delta(),
                                i.time,
                                i.stable_dt,
                            )
                        });
                        let pointer_pos = ctx.pointer_latest_pos();

                        for row in rows.iter() {
                            let is_draft = row.id.is_none();
                            let locked = row.locked;
                            let source_locked = row.source_locked;
                            let selected = row.id == current_id;
                            let width = (ui.available_width() - 8.0).max(60.0);
                            // 只声明 click 语义：长按手势由本文件自建状态机驱动，
                            // 不依赖 egui 的拖动判定（§1，ADR-0005）。
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::click());
                            if !is_draft {
                                row_rects.push(rect);
                            }

                            // 锁图标：行右端固定图标道内的一块独占命中区（§3，ADR-0007）。
                            // 命中该区域时本行的选中、内联编辑、长按拖动全部让位。
                            let lock_rect = sidebar_row_lock_rect(rect);
                            let pointer_on_lock =
                                pointer_pos.map_or(false, |pos| lock_rect.contains(pos));

                            // 标题子矩形的命中判定；绘制与判定共用同一函数，保证一致（§2）。
                            let text_rect = sidebar_row_text_rect(rect);
                            let pointer_on_title =
                                pointer_pos.map_or(false, |pos| text_rect.contains(pos));
                            let editing_this = self
                                .editing_title
                                .as_ref()
                                .map_or(false, |edit| edit.target == row.id);
                            let dragging_this = self
                                .row_drag
                                .as_ref()
                                .map_or(false, |drag| Some(drag.row_id) == row.id);
                            let ghosting_this = dragging_this
                                && self
                                    .row_drag
                                    .as_ref()
                                    .map_or(false, |drag| drag.phase == LongPress::Armed);

                            // 锁图标点击：独占命中区，先于本行的选中/编辑/拖动判定。
                            if !is_draft && !editing_this {
                                let lock_response = ui.interact(
                                    lock_rect,
                                    Id::new(("lock", row.id)),
                                    Sense::click(),
                                );
                                if lock_response.clicked() {
                                    if source_locked {
                                        unlock_blocked = true;
                                    } else {
                                        lock_toggle =
                                            Some((row.id.expect("非草稿行必有 id"), !locked));
                                    }
                                }
                                if lock_response.hovered() {
                                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                lock_response.on_hover_text(if source_locked {
                                    "提示词来自图片，无法解锁"
                                } else if locked {
                                    "点击解锁"
                                } else {
                                    "点击上锁"
                                });
                            }

                            if is_draft {
                                // 草稿行本来就是当前示例：点标题进改名，点别处什么都不做。
                                // 原来这里无条件 select(None)，非空草稿会被判成「切换示例」，
                                // 于是点一下自己就弹「草稿尚未保存」。
                                if response.clicked() && !editing_this {
                                    if selected && pointer_on_title && self.row_drag.is_none() {
                                        edit_start = Some(row.id);
                                    }
                                }
                            } else {
                                let row_id = row.id.expect("非草稿行必有 id");
                                if dragging_this {
                                    source_rect = Some(rect);
                                }
                                // 手势起点 = 指针物理按下落在本行（§1，ADR-0005）；
                                // 正在内联编辑的行禁止发起拖动（§1 互斥）。
                                // 只认原始指针状态：egui 在按住超过 max_click_duration(0.8s)
                                // 后会清空 potential_click_id，is_pointer_button_down_on 随之变假
                                // （egui-0.36.2/src/input_state/mod.rs:1488、interaction.rs:229）。
                                if down_on.is_none()
                                    && pressed_now
                                    && press_origin.is_some_and(|origin| rect.contains(origin))
                                    && !editing_this
                                    && !pointer_on_lock
                                {
                                    down_on = Some((row_id, rect));
                                }
                                // 兜底：按下与松手落在同一帧时状态机来不及建立，按普通点击处理。
                                if response.clicked()
                                    && self.row_drag.is_none()
                                    && !editing_this
                                    && !pointer_on_lock
                                {
                                    if selected && pointer_on_title {
                                        edit_start = Some(row.id);
                                    } else {
                                        clicked = Some(row.id);
                                    }
                                }
                            }

                            // 当前行悬停标题：光标变 I-beam；拖动中不抢光标（§1、§2）。
                            if selected
                                && pointer_on_title
                                && !editing_this
                                && !pointer_on_lock
                                && self.row_drag.is_none()
                            {
                                ctx.set_cursor_icon(egui::CursorIcon::Text);
                            }

                            paint_sidebar_row(
                                ui,
                                rect,
                                selected,
                                response.hovered(),
                                &row.title,
                                row.thumb.as_ref(),
                                &images_dir,
                                &mut self.cache,
                                is_draft,
                                editing_this,
                                if is_draft {
                                    None
                                } else if locked {
                                    Some(true)
                                } else if response.hovered() || pointer_on_lock {
                                    Some(false)
                                } else {
                                    None
                                },
                                source_locked,
                                if ghosting_this {
                                    DRAG_SOURCE_OPACITY
                                } else {
                                    1.0
                                },
                                false,
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

                        // ---- 长按手势状态机（§1，ADR-0005）：从物理按下计时 ----
                        let row_centers: Vec<f32> =
                            row_rects.iter().map(|rect| rect.center().y).collect();
                        if pointer_down {
                            if self.row_drag.is_none() {
                                if let Some((row_id, rect)) = down_on {
                                    self.row_drag = Some(RowDrag {
                                        row_id,
                                        phase: LongPress::Waiting,
                                        displacement: 0.0,
                                        displacement_at_arm: 0.0,
                                        grab_offset: press_origin
                                            .map_or(Vec2::ZERO, |pos| pos - rect.min),
                                        row_size: rect.size(),
                                        insert: None,
                                        last_pointer: press_origin.unwrap_or(rect.center()),
                                    });
                                }
                            }
                            if let Some(drag) = self.row_drag.as_mut() {
                                drag.displacement = drag_delta.map_or(0.0, |delta| delta.length());
                                let elapsed =
                                    press_start.map_or(0.0, |start| (now - start).max(0.0));
                                let phase = long_press_transition(
                                    drag.phase,
                                    Duration::from_secs_f64(elapsed),
                                    drag.displacement,
                                );
                                if phase == LongPress::Armed && drag.phase != LongPress::Armed {
                                    drag.displacement_at_arm = drag.displacement;
                                }
                                drag.phase = phase;

                                // 达标即反馈：280ms 一到（即使指针未动）就出虚影并实时重算落点。
                                if drag.phase == LongPress::Armed {
                                    if let Some(pos) = pointer_pos {
                                        drag.last_pointer = pos;
                                    }
                                    drag.insert = Some(drag_insertion_index(
                                        drag.last_pointer.y,
                                        viewport.top(),
                                        viewport.bottom(),
                                        &row_centers,
                                    ));
                                    // 热区自动滚动：指针静止也持续滚；指针无坐标时停（§1、§3）。
                                    if pointer_pos.is_some() {
                                        let speed = autoscroll_speed(
                                            drag.last_pointer.y,
                                            viewport.top(),
                                            viewport.bottom(),
                                        );
                                        if speed != 0.0 {
                                            ui.scroll_with_delta(Vec2::new(0.0, -speed * dt));
                                        }
                                    }
                                }
                            }
                        } else if let Some(drag) = self.row_drag.take() {
                            if released {
                                let destination_changes = drag.insert.map_or(false, |insert| {
                                    drag_source_index(&rows, drag.row_id)
                                        .and_then(|source| {
                                            reorder_destination(source, insert, row_rects.len())
                                        })
                                        .is_some()
                                });
                                let (outcome, insert) = gesture_outcome(
                                    drag.phase,
                                    drag.moved(),
                                    destination_changes,
                                    drag.insert,
                                );
                                release = Some((drag.row_id, outcome, insert));
                            }
                        }

                        // 插入蓝线：只在真正会改变顺序的落点显示，且画在虚影下层（§1、§2）。
                        if let Some(drag) = self.row_drag.as_ref() {
                            if drag.phase == LongPress::Armed && drag.moved() {
                                let changes = drag.insert.map_or(false, |insert| {
                                    drag_source_index(&rows, drag.row_id)
                                        .and_then(|source| {
                                            reorder_destination(source, insert, row_rects.len())
                                        })
                                        .is_some()
                                });
                                if changes {
                                    if let Some(insert) = drag.insert {
                                        if let Some(y) = insertion_line_y(insert, &row_rects) {
                                            let x_range = row_rects
                                                .first()
                                                .map(|rect| rect.x_range())
                                                .unwrap_or_else(|| ui.min_rect().x_range());
                                            ui.painter().hline(
                                                x_range,
                                                y,
                                                Stroke::new(
                                                    2.0,
                                                    theme::Palette::current(ui.ctx()).accent,
                                                ),
                                            );
                                        }
                                    }
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
                // 松手派发（§1）：达标 + 已移动 + 落点改变顺序才重排；否则按选中处理。
                if let Some((row_id, outcome, insert)) = release {
                    match outcome {
                        GestureOutcome::None => {}
                        GestureOutcome::Click => {
                            let on_title = ctx
                                .pointer_latest_pos()
                                .zip(source_rect)
                                .map_or(false, |(pos, rect)| {
                                    sidebar_row_text_rect(rect).contains(pos)
                                });
                            let locked = rows
                                .iter()
                                .find(|row| row.id == Some(row_id))
                                .map_or(false, |row| row.locked);
                            // 手动锁定示例只能选中查看，不进内联编辑（ADR-0007）；
                            // 来源锁不拦改名（ADR-0012）。
                            let source_locked = rows
                                .iter()
                                .find(|row| row.id == Some(row_id))
                                .map_or(false, |row| row.source_locked);
                            if (!locked || source_locked) && Some(row_id) == current_id && on_title {
                                self.begin_inline_edit(Some(row_id));
                            } else {
                                self.select(Some(row_id));
                            }
                        }
                        GestureOutcome::Select => self.select(Some(row_id)),
                        GestureOutcome::Reorder => {
                            if let Some(insert) = insert {
                                insert_at = Some((row_id, insert));
                            }
                        }
                    }
                }

                if let Some(target) = clicked {
                    self.select(target);
                }
                if let Some((from, insert)) = insert_at {
                    self.move_row(from, insert);
                }
                // 锁开关写库（ADR-0007）：界面层同步内存，数据库层强制拒绝修改/删除。
                // 上锁前先把当前示例的未写改动 flush 掉，否则锁上后这些改动再也写不进去（ADR-0006）。
                if let Some((id, locked)) = lock_toggle {
                    // flush 失败（磁盘满/文件占用）：取消这次上锁，保留可重试的机会。
                    let ready = !locked || self.flush_now();
                    match ready.then(|| self.store.set_locked(id, locked)) {
                        Some(Err(err)) => self.set_error(err),
                        Some(Ok(())) => {
                            for example in [self.current.as_mut(), self.baseline.as_mut()]
                                .into_iter()
                                .flatten()
                            {
                                if example.id == Some(id) {
                                    example.locked = locked;
                                }
                            }
                            if locked
                                && self
                                    .editing_title
                                    .as_ref()
                                    .map_or(false, |edit| edit.target == Some(id))
                            {
                                self.editing_title = None;
                            }
                            self.set_status(
                                &ctx,
                                if locked {
                                    "已锁定示例".to_string()
                                } else {
                                    "已解锁示例".to_string()
                                },
                            );
                        }
                        None => {}
                    }
                }
                if unlock_blocked {
                    self.set_status(&ctx, "提示词来自图片，无法解锁".to_string());
                }

                // 拖动虚影与源行淡占位（§2）：Foreground 图层整行副本 + 投影。
                self.paint_drag_ghost(ui, &rows, source_rect, &images_dir);
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
                    // 来源锁示例的 update() 会被锁校验挡下，改名走单独的写口（ADR-0012）。
                    let result = if stored.prompt_source.is_locked_source() {
                        self.store.rename(id, &title)
                    } else {
                        self.store.update(&mut stored)
                    };
                    match result {
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

    /// 画拖动虚影与被拖源行的淡占位（§2）。
    ///
    /// 虚影画在 `Order::Foreground` 的独立图层：既不被 `ScrollArea` 裁剪，
    /// 也盖住相邻行；插入蓝线画在 `Middle` 图层，因此天然位于虚影下层。
    fn paint_drag_ghost(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[SidebarRow],
        source_rect: Option<Rect>,
        images_dir: &Path,
    ) {
        let armed = self
            .row_drag
            .as_ref()
            .map_or(false, |drag| drag.phase == LongPress::Armed);
        // 达标瞬间的约 120ms 淡入 / 上浮（§2）：未达标时目标为 false，下次拖动从头播。
        let uplift = ui
            .ctx()
            .animate_bool_with_time(Id::new("row_drag_ghost_uplift"), armed, 0.12)
            * DRAG_GHOST_UPLIFT;
        if !armed {
            return;
        }
        let Some(drag) = self.row_drag.as_ref() else {
            return;
        };
        let row_id = drag.row_id;
        let grab_offset = drag.grab_offset;
        let row_size = drag.row_size;
        let pointer = drag.last_pointer;
        let selected = self.current_id() == Some(row_id);
        let Some(row) = rows.iter().find(|row| row.id == Some(row_id)) else {
            return;
        };
        let title = row.title.clone();
        let thumb = row.thumb.as_ref();

        // 源行原位：不隐藏、列表不塌陷，只变淡到约 35%（§2）。
        if let Some(rect) = source_rect {
            paint_sidebar_row(
                ui,
                rect,
                selected,
                false,
                &title,
                thumb,
                images_dir,
                &mut self.cache,
                false,
                false,
                None,
                row.source_locked,
                DRAG_SOURCE_OPACITY,
                false,
            );
        }

        // 1:1 跟手：虚影左上角 = 指针 − 按下时的行内偏移 − 上浮量（§2）。
        let ghost_rect =
            Rect::from_min_size(pointer - grab_offset - Vec2::new(0.0, uplift), row_size);
        let ctx = ui.ctx().clone();
        egui::Area::new(Id::new("row_drag_ghost"))
            .order(egui::Order::Foreground)
            .interactable(false)
            .fixed_pos(ghost_rect.min)
            .show(&ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(ghost_rect.size(), Sense::hover());
                paint_sidebar_row(
                    ui,
                    rect,
                    selected,
                    false,
                    &title,
                    thumb,
                    images_dir,
                    &mut self.cache,
                    false,
                    false,
                    None,
                    row.source_locked,
                    DRAG_GHOST_OPACITY,
                    true,
                );
            });
    }

    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let p = theme::Palette::current(&ctx);
        let mut actions: Vec<Action> = Vec::new();
        egui::Panel::bottom("bottom_bar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let draft = self
                    .current
                    .as_ref()
                    .map(|example| example.is_draft())
                    .unwrap_or(true);
                let dirty = self.dirty();
                let locked = self
                    .current
                    .as_ref()
                    .map(|example| example.locked)
                    .unwrap_or(false);
                if draft {
                    if ui
                        .add_enabled(self.current.is_some(), egui::Button::new("保存为新示例"))
                        .on_hover_text("草稿落库：此时才把图片复制入库")
                        .clicked()
                    {
                        actions.push(Action::Save);
                    }
                } else if ui
                    .add_enabled(dirty && !locked, egui::Button::new("保存"))
                    .on_hover_text(if locked {
                        "该示例已锁定，无法修改"
                    } else {
                        "Ctrl+S"
                    })
                    .clicked()
                {
                    actions.push(Action::Save);
                }
                let source_locked = self.current_source_locked();
                if ui
                    .add_enabled(
                        self.current_id().is_some() && (!locked || source_locked),
                        egui::Button::new("删除"),
                    )
                    .on_hover_text(if locked && !source_locked {
                        "该示例已锁定，无法删除"
                    } else {
                        "删除当前示例（托管图片文件保留）"
                    })
                    .clicked()
                {
                    actions.push(Action::Delete);
                }

                ui.separator();
                let status_text = self.status_message(ui.ctx());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    // 最右：纯文本版本号，不可点、不参与任何动作（D9）。
                    let version = env!("CARGO_PKG_VERSION");
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("v{version}"))
                                .color(p.text_muted)
                                .size(12.0),
                        )
                        .selectable(false),
                    );
                    // 版本号左边：仅「非空草稿」时提示尚未落库。
                    let draft_unsaved = self
                        .current
                        .as_ref()
                        .map(|example| example.is_draft() && !example.is_blank())
                        .unwrap_or(false);
                    if draft_unsaved {
                        ui.add(
                            egui::Label::new(
                                RichText::new("草稿未保存").color(p.text_muted).size(12.0),
                            )
                            .truncate(),
                        )
                        .on_hover_text("草稿只在保存为新示例后才落库");
                    }
                    // 状态文案占据剩余宽度，窄窗口下自适应截断。
                    let width = ui.available_width().max(0.0);
                    let height = ui.text_style_height(&egui::TextStyle::Body).max(16.0);
                    ui.add_sized(
                        [width, height],
                        egui::Label::new(
                            RichText::new(status_text).color(p.text_muted).size(12.0),
                        )
                        .truncate(),
                    );
                });
            });
            ui.add_space(4.0);
        });

        for action in actions {
            self.run_action(&ctx, action);
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

    /// 中央区：图片框 + 正/负向提示词框，按图片朝向自适应排布。
    ///
    /// 宽图（含正方形、无图）走横版：上图片、下分左右两段提示词；
    /// 高图走竖版：左图片、右上正向、右下负向。目标是尽量露出图片的长边。
    fn central(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let avail = ui.available_rect_before_wrap();
            let aspect = self.cached_image_aspect(ui.ctx());
            let layout = central_layout(avail, aspect);
            self.image_box(ui, layout.image);
            self.prompt_box(ui, layout.positive, PromptKind::Positive);
            self.prompt_box(ui, layout.negative, PromptKind::Negative);
            // 三块都是按显式矩形绘制的：这里补一次分配，让面板认为自己已用完空间。
            let _ = ui.allocate_space(avail.size());
        });
    }

    /// 当前图片的宽高比：优先待入库图，其次已入库图；都没有则 `None`（走横版）。
    ///
    /// 纹理已经缓存，重复调用只是在查缓存；首次调用顺带解码一次预览。
    fn cached_image_aspect(&mut self, ctx: &egui::Context) -> Option<f32> {
        if let Some(pending) = self.pending_image.as_ref() {
            let key = pending.key.clone();
            let path = pending.path.clone();
            return self.cache.preview(ctx, &key, &path).ok().map(|c| c.aspect());
        }
        let file_name = self.current.as_ref().and_then(|e| e.image_file_name())?;
        let path = self.store.images_dir().join(&file_name);
        self.cache
            .preview(ctx, &file_name, &path)
            .ok()
            .map(|c| c.aspect())
    }

    fn image_box(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let p = theme::Palette::current(ui.ctx());
        let response = ui.interact(rect, Id::new("image_box"), Sense::click());
        // 锁定示例：图片框不响应点击、不接受拖入（ADR-0007）。
        let locked = self
            .current
            .as_ref()
            .map(|example| example.locked)
            .unwrap_or(false);
        let hovered = response.hovered() && !locked;
        let pointer_over = ui
            .ctx()
            .pointer_hover_pos()
            .map_or(false, |pos| rect.contains(pos));
        // 拖入文件时高亮整框（§1）。
        let file_drop_hover =
            ui.ctx().input(|i| !i.raw.hovered_files.is_empty()) && pointer_over && !locked;
        let stroke_color = if file_drop_hover {
            p.accent
        } else if hovered {
            p.border_strong
        } else {
            p.border
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(6),
            p.surface_sunken,
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
        if !locked && (remove.hovered() || remove.clicked()) {
            ui.painter().rect_filled(
                remove_rect,
                CornerRadius::same(4),
                p.row_hover,
            );
            ui.painter().text(
                remove_rect.center(),
                egui::Align2::CENTER_CENTER,
                "×",
                egui::FontId::proportional(14.0),
                p.text,
            );
        }

        let mut hover_text: Option<String> = None;
        if locked {
            hover_text = Some("🔒 该示例已锁定".to_string());
        } else if remove.clicked() {
            self.pending_image = None;
            if let Some(example) = self.current.as_mut() {
                example.image_hash = None;
                example.image_ext = None;
            }
            self.cache.drop_preview();
            // 已落库示例没有「保存时刻」：移除引用后立即写库（ADR-0006）。
            self.flush_now();
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
                            p.text,
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
                            p.text,
                        );
                    }
                }
            }
            (None, None) => {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    if locked {
                        "🔒 该示例已锁定"
                    } else {
                        "点击或拖拽本地图片到此处"
                    },
                    egui::FontId::proportional(14.0),
                    p.text_faint,
                );
            }
        }

        // 已有图片时叠一层半透明提示，鼠标移入才显示「点击更换图片」（§1）。
        if hovered && (pending.is_some() || managed.is_some()) {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(6),
                p.backdrop,
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "点击更换图片",
                egui::FontId::proportional(14.0),
                Color32::WHITE,
            );
        }

        if let Some(text) = hover_text {
            response.on_hover_text(text);
        }
    }

    /// 一个提示词文本框：外框 + 左上角小标题/锁标记 + 右上角复制按钮 + 可滚动正文。
    ///
    /// 正/负向同权（ADR-0011）：各自独立复制、独立自动保存，锁定时一起只读。
    fn prompt_box(&mut self, ui: &mut egui::Ui, rect: Rect, kind: PromptKind) {
        let p = theme::Palette::current(ui.ctx());
        let ctx = ui.ctx().clone();
        ui.painter().rect(
            rect,
            CornerRadius::same(6),
            p.surface_sunken,
            Stroke::new(1.0, p.border),
            StrokeKind::Inside,
        );

        let Some(example) = self.current.as_ref() else {
            return;
        };
        let locked = example.locked;
        let source_locked = example.prompt_source.is_locked_source();
        let copy_text = match kind {
            PromptKind::Positive => example.positive.trim().to_string(),
            PromptKind::Negative => example.negative.trim().to_string(),
        };
        let is_stored = !example.is_draft();

        // 顶部一条：左侧小标题（锁定时常显 🔒，来源锁用强调色），右侧复制按钮。
        let head_top = rect.top() + 6.0;
        let head_center = head_top + COPY_BTN_H * 0.5;
        let mut title_left = rect.left() + 8.0;
        if locked {
            ui.painter().text(
                egui::pos2(title_left, head_center),
                Align2::LEFT_CENTER,
                "🔒",
                FontId::proportional(12.0),
                if source_locked { p.accent } else { p.text_muted },
            );
            title_left += 16.0;
        }
        ui.painter().text(
            egui::pos2(title_left, head_center),
            Align2::LEFT_CENTER,
            kind.title(),
            FontId::proportional(11.0),
            p.text_muted,
        );

        // 复制按钮：自绘命中区，宽度固定为「已复制」以避开反馈时的抖动。
        let copy_rect = Rect::from_min_size(
            egui::pos2(rect.right() - 8.0 - COPY_BTN_W, head_top),
            Vec2::new(COPY_BTN_W, COPY_BTN_H),
        );
        let now = ctx.input(|i| i.time);
        let flashing = self
            .copy_flash
            .map_or(false, |(flashed, until)| flashed == kind && now < until);
        let can_copy = !copy_text.is_empty();
        let copy_response = ui.interact(
            copy_rect,
            Id::new(("copy_prompt", kind.key())),
            Sense::click(),
        );
        let copy_hovered = can_copy && copy_response.hovered();
        if copy_hovered {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
            ui.painter()
                .rect_filled(copy_rect, CornerRadius::same(4), p.row_hover);
        }
        // 三态：空文本降低对比（向背景靠拢，深浅两个主题都看不出可点）、常态 muted、悬停提亮。
        let copy_color = if !can_copy {
            p.text_muted.gamma_multiply(0.55)
        } else if flashing {
            p.accent
        } else if copy_hovered {
            p.text
        } else {
            p.text_muted
        };
        ui.painter().text(
            copy_rect.center(),
            Align2::CENTER_CENTER,
            if flashing { "已复制" } else { "复制" },
            FontId::proportional(12.0),
            copy_color,
        );
        if can_copy {
            if copy_response.on_hover_text("复制这一侧的提示词").clicked() {
                ctx.copy_text(copy_text);
                self.copy_flash = Some((kind, now + COPY_FLASH));
                ctx.request_repaint_after(Duration::from_secs_f64(COPY_FLASH));
                self.set_status(&ctx, format!("{}已复制到剪贴板", kind.title()));
            }
        }

        // 正文：从按钮下方开始，自己滚动；「一个框」的观感由外框保证。
        let text_rect = Rect::from_min_max(
            egui::pos2(rect.left() + 8.0, head_top + COPY_BTN_H + 6.0),
            egui::pos2(rect.right() - 8.0, rect.bottom() - 8.0),
        );
        if text_rect.width() < 16.0 || text_rect.height() < 16.0 {
            return;
        }
        let edit_id = Id::new(("prompt_edit", kind.key()));
        let mut changed = false;
        if let Some(example) = self.current.as_mut() {
            let text = match kind {
                PromptKind::Positive => &mut example.positive,
                PromptKind::Negative => &mut example.negative,
            };
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(text_rect)
                    .layout(Layout::top_down(Align::Min))
                    .id_salt(("prompt_box", kind.key())),
                |ui| {
                    ui.set_clip_rect(text_rect);
                    ScrollArea::vertical()
                        .id_salt(("prompt_scroll", kind.key()))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            changed |= ui
                                .add_enabled(
                                    !locked,
                                    TextEdit::multiline(text)
                                        .id(edit_id)
                                        .desired_width(ui.available_width())
                                        .desired_rows(6)
                                        .frame(Frame::NONE)
                                        .hint_text(if kind == PromptKind::Negative {
                                            "（可留空）"
                                        } else {
                                            "（留空）"
                                        }),
                                )
                                .changed();
                        });
                },
            );
        }

        // 已落库示例：输入停止 500ms 后自动写库；失焦即落库（ADR-0006）。
        // egui 是反应式的：不请求一次将来的重绘，到点那一帧永远不会到来。
        if is_stored && !locked {
            if changed {
                self.autosave_deadline = Some(now + AUTOSAVE_DEBOUNCE.as_secs_f64());
                ctx.request_repaint_after(AUTOSAVE_DEBOUNCE);
            } else {
                let focused = ui
                    .memory(|memory| memory.focused())
                    .map_or(false, |id| id == edit_id);
                if !focused && self.dirty() {
                    self.flush_now();
                }
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let p = theme::Palette::current(ctx);
        // Esc 与「取消」同义：只关闭当前弹窗，不做任何改动。
        let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));

        // 未保存改动确认。
        if self.pending_select.is_some() {
            let mut save = false;
            let mut discard = false;
            let mut cancel = false;
            theme::modal_shell(ctx, "unsaved_modal", 320.0, |ui| {
                theme::modal_title(ui, "草稿尚未保存");
                theme::modal_desc(ui, "当前草稿还没有保存为示例，要如何处理？");
                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::primary_button(ui, "保存为示例").clicked() {
                        save = true;
                    }
                    if theme::secondary_button(ui, "放弃草稿").clicked() {
                        discard = true;
                    }
                    if theme::secondary_button(ui, "取消").clicked() {
                        cancel = true;
                    }
                });
            });
            if save {
                self.save_current();
                self.perform_pending_select();
            } else if discard {
                self.perform_pending_select();
            } else if cancel || escape {
                self.pending_select = None;
            }
        }

        // 关窗确认：只有非空草稿会走到这里（ADR-0006 D9）。
        if self.close_confirm {
            let mut discard = false;
            let mut cancel = false;
            theme::modal_shell(ctx, "close_modal", 320.0, |ui| {
                theme::modal_title(ui, "草稿尚未保存");
                theme::modal_desc(ui, "当前草稿还没有保存为新示例，关窗后改动会丢失。");
                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::secondary_button(ui, "放弃草稿").clicked() {
                        discard = true;
                    }
                    if theme::secondary_button(ui, "取消").clicked() {
                        cancel = true;
                    }
                });
            });
            if discard {
                // 用户确认放弃：记住已确认，直接请求关窗。
                self.close_confirm = false;
                self.close_confirmed = true;
                self.current = None;
                self.pending_image = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if cancel || escape {
                self.close_confirm = false;
            }
        }

        // 关于（D4）：只有名称与版本号。
        if self.about_open {
            let mut close = false;
            let version = env!("CARGO_PKG_VERSION");
            theme::modal_shell(ctx, "about_modal", 340.0, |ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new(APP_TITLE)
                            .size(18.0)
                            .strong()
                            .color(p.text_strong),
                    )
                    .selectable(false),
                );
                ui.add_space(4.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("v{version}"))
                            .size(13.0)
                            .color(p.text_muted),
                    )
                    .selectable(false),
                );
                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::secondary_button(ui, "关闭").clicked() {
                        close = true;
                    }
                });
            });
            if close || escape {
                self.about_open = false;
            }
        }

        // 删除确认。
        if self.delete_confirm {
            let mut confirm = false;
            let mut close = false;
            let name = self
                .current
                .as_ref()
                .map(|example| example.display_title().to_string())
                .unwrap_or_default();
            theme::modal_shell(ctx, "delete_modal", 340.0, |ui| {
                theme::modal_title(ui, "删除这条示例？");
                theme::modal_desc(ui, format!("「{name}」的标题与提示词将被删除。"));
                theme::modal_desc(ui, "托管目录里的图片文件会保留（可能被其它示例共用）。");
                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::danger_button(ui, "删除").clicked() {
                        confirm = true;
                    }
                    if theme::secondary_button(ui, "取消").clicked() {
                        close = true;
                    }
                });
            });
            if confirm {
                self.delete_current();
                close = true;
            }
            if close || escape {
                self.delete_confirm = false;
            }
        }

        // 图片内提示词覆盖确认（ADR-0013）：确认前图片与文本都不动，取消即完全回退。
        if self.prompt_confirm.is_some() {
            let preview = self
                .prompt_confirm
                .as_ref()
                .and_then(|choice| match &choice.detection {
                    Detection::Found { positive, .. } => Some(preview_excerpt(positive, 80)),
                    _ => None,
                })
                .unwrap_or_default();
            let mut confirm = false;
            let mut cancel = false;
            theme::modal_shell(ctx, "prompt_confirm_modal", 420.0, |ui| {
                theme::modal_title(ui, "覆盖提示词？");
                theme::modal_desc(ui, "确定清空文本框，操作不可逆");
                ui.add_space(10.0);
                ui.add(
                    egui::Label::new(RichText::new(preview).size(12.0).color(p.text_muted))
                        .wrap(),
                );
                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::danger_button(ui, "确定").clicked() {
                        confirm = true;
                    }
                    if theme::secondary_button(ui, "取消").clicked() {
                        cancel = true;
                    }
                });
            });
            if confirm {
                if let Some(choice) = self.prompt_confirm.take() {
                    self.commit_image_choice(ctx, choice);
                }
            } else if cancel || escape {
                self.prompt_confirm = None;
                self.set_status(ctx, "已取消：图片与文本都没有改动".to_string());
            }
        }

        // 导出：列表逐条勾选，默认全选（D7/D8）。
        if self.export_open {
            let mut close = false;
            let mut do_export = false;
            let mut browse = false;
            theme::modal_shell(ctx, "export_modal", 560.0, |ui| {
                theme::modal_title(ui, "导出图库");
                let rows: Vec<(i64, String)> = self
                    .store
                    .rows()
                    .iter()
                    .map(|row| (row.id, row.display_title().to_string()))
                    .collect();
                let selected_count = rows
                    .iter()
                    .filter(|(id, _)| self.export_selected.contains(id))
                    .count();
                theme::modal_desc(
                    ui,
                    format!("已选择 {selected_count} 条示例导出图库"),
                );
                ui.add_space(12.0);

                egui::Frame::new()
                    .fill(p.surface)
                    .stroke(Stroke::new(1.0, p.border))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ScrollArea::vertical()
                            .id_salt("export_list")
                            .max_height(260.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                let all = !rows.is_empty() && selected_count == rows.len();
                                let partial = selected_count > 0 && !all;

                                // 首行：全选 / 清空。
                                let (rect, response) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width(), 32.0),
                                    Sense::click(),
                                );
                                if response.hovered() {
                                    ui.painter().rect_filled(
                                        rect,
                                        CornerRadius::same(4),
                                        p.row_hover,
                                    );
                                }
                                let checkbox = Rect::from_min_size(
                                    rect.min + Vec2::new(8.0, 7.0),
                                    Vec2::splat(18.0),
                                );
                                export_checkbox(ui.painter(), checkbox, all, partial, &p);
                                ui.painter().text(
                                    egui::pos2(checkbox.right() + 10.0, rect.center().y),
                                    Align2::LEFT_CENTER,
                                    "全选",
                                    FontId::proportional(13.0),
                                    p.text,
                                );
                                if response.clicked() {
                                    if all {
                                        self.export_selected.clear();
                                    } else {
                                        self.export_selected =
                                            rows.iter().map(|(id, _)| *id).collect();
                                    }
                                }

                                for (id, title) in &rows {
                                    let checked = self.export_selected.contains(id);
                                    let (rect, response) = ui.allocate_exact_size(
                                        Vec2::new(ui.available_width(), 32.0),
                                        Sense::click(),
                                    );
                                    if response.hovered() {
                                        ui.painter().rect_filled(
                                            rect,
                                            CornerRadius::same(4),
                                            p.row_hover,
                                        );
                                    }
                                    let checkbox = Rect::from_min_size(
                                        rect.min + Vec2::new(8.0, 7.0),
                                        Vec2::splat(18.0),
                                    );
                                    export_checkbox(
                                        ui.painter(),
                                        checkbox,
                                        checked,
                                        false,
                                        &p,
                                    );
                                    let galley = ui.painter().layout_no_wrap(
                                        title.clone(),
                                        FontId::proportional(13.0),
                                        p.text,
                                    );
                                    ui.painter().galley(
                                        egui::pos2(
                                            checkbox.right() + 10.0,
                                            rect.center().y - galley.size().y * 0.5,
                                        ),
                                        galley,
                                        p.text,
                                    );
                                    if response.clicked() {
                                        if checked {
                                            self.export_selected.remove(id);
                                        } else {
                                            self.export_selected.insert(*id);
                                        }
                                    }
                                }
                            });
                    });

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("导出地址").size(12.0).color(p.text_muted));
                    ui.add(
                        TextEdit::singleline(&mut self.export_path)
                            .desired_width(380.0)
                            .hint_text("导出路径"),
                    );
                    if theme::secondary_button(ui, "浏览…").clicked() {
                        browse = true;
                    }
                });

                let live_count = rows
                    .iter()
                    .filter(|(id, _)| self.export_selected.contains(id))
                    .count();
                ui.add_space(18.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if theme::primary_button_enabled(ui, "导出", live_count > 0).clicked() {
                        do_export = true;
                    }
                    if theme::secondary_button(ui, "取消").clicked() {
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
            } else if close || escape {
                self.export_open = false;
            }
        }
    }
}

/// 顶部菜单与底栏按钮共用的动作，由闭包收集后在外层统一执行（避免与 self 的借用纠缠）。
#[derive(Clone, Copy)]
enum Action {
    Save,
    Delete,
    Export,
    Import,
    OpenDataDir,
}

// ---------------------------------------------------------------- 自定义控件

/// 提示词框的两侧（ADR-0011）：正向与负向同权，各自独立复制、独立自动保存。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromptKind {
    Positive,
    Negative,
}

impl PromptKind {
    /// 用作控件 id 后缀与滚动区盐值。
    fn key(self) -> &'static str {
        match self {
            Self::Positive => "positive",
            Self::Negative => "negative",
        }
    }

    /// 框内小标题与状态文案用的名称。
    fn title(self) -> &'static str {
        match self {
            Self::Positive => "正向提示词",
            Self::Negative => "负向提示词",
        }
    }
}

/// 中央区三块画在哪儿（纯几何，便于单测）。
#[derive(Clone, Copy, Debug, PartialEq)]
struct CentralLayout {
    image: Rect,
    positive: Rect,
    negative: Rect,
    /// 是否采用竖版（左图 + 右侧两段文字）。
    portrait: bool,
}

/// 按可用矩形与图片像素宽高比算出中央区排版。
///
/// 只有 `aspect < 1.0` 才考虑竖版；正方形（1.0）与无图都走横版。
/// 竖版有两道安全网：图片列 ≥ 240pt、文本列 ≥ 320pt，任一不满足就退回横版，
/// 避免窄窗口里两块文本被压成一条缝。
fn central_layout(avail: Rect, aspect: Option<f32>) -> CentralLayout {
    let tall = aspect.map_or(false, |aspect| aspect < 1.0);
    let text_col_w = avail.width() * (1.0 - IMAGE_SHARE) - CENTRAL_GAP;
    let portrait =
        tall && avail.width() * IMAGE_SHARE >= MIN_IMAGE_COL_W && text_col_w >= MIN_TEXT_COL_W;

    if portrait {
        let image_w = (avail.width() * IMAGE_SHARE).max(MIN_IMAGE_COL_W);
        let image = Rect::from_min_size(avail.min, Vec2::new(image_w, avail.height()));
        let text_x = avail.min.x + image_w + CENTRAL_GAP;
        let text_w = (avail.max.x - text_x).max(1.0);
        let text_h = (avail.height() - CENTRAL_GAP).max(1.0);
        let positive_h = text_h * POSITIVE_SHARE;
        let positive =
            Rect::from_min_size(egui::pos2(text_x, avail.min.y), Vec2::new(text_w, positive_h));
        let negative = Rect::from_min_size(
            egui::pos2(text_x, avail.min.y + positive_h + CENTRAL_GAP),
            Vec2::new(text_w, (text_h - positive_h).max(1.0)),
        );
        CentralLayout {
            image,
            positive,
            negative,
            portrait: true,
        }
    } else {
        // 横版：图片在上吃掉可用高的 42%（沿用 0.1.x 的观感），下方一条横带 62:38 分给正/负向。
        // 横向留长边：宽图的长边是宽，所以图片占满整行、文本在它下面左右排。
        let image_h = (avail.height() * IMAGE_SHARE).clamp(140.0, 420.0);
        let image = Rect::from_min_size(avail.min, Vec2::new(avail.width(), image_h));
        let rest_y = avail.min.y + image_h + CENTRAL_GAP;
        let rest_h = (avail.max.y - rest_y).max(1.0);
        let positive_w = (avail.width() - CENTRAL_GAP).max(1.0) * POSITIVE_SHARE;
        let positive = Rect::from_min_size(
            egui::pos2(avail.min.x, rest_y),
            Vec2::new(positive_w, rest_h),
        );
        let negative_x = avail.min.x + positive_w + CENTRAL_GAP;
        let negative = Rect::from_min_size(
            egui::pos2(negative_x, rest_y),
            Vec2::new((avail.max.x - negative_x).max(1.0), rest_h),
        );
        CentralLayout {
            image,
            positive,
            negative,
            portrait: false,
        }
    }
}

// ---------------------------------------------------------------- 侧边栏行

/// 已读入内存、等待用户确认「图片内提示词会清空现有文本」的选图结果。
///
/// 字节在确认前就已读好：确认弹窗开着时源文件被改动/删除也不影响这次操作。
struct PendingChoice {
    path: PathBuf,
    /// 源文件名，仅用于状态与悬停提示。
    label: String,
    /// 纹理缓存 key；同一张图在多次重绘间保持稳定。
    key: String,
    bytes: Vec<u8>,
    ext: &'static str,
    size: Option<(u32, u32)>,
    /// ComfyUI 提示词检测结果（ADR-0013）。
    detection: Detection,
}

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
    /// 只读锁状态（ADR-0007）；草稿行恒为 false。
    locked: bool,
    /// 提示词来源锁（ADR-0012）：为真时不可解锁，图标用强调色。
    source_locked: bool,
}

/// 侧边栏缩略图的来源（快照，避免与 store 的可变借用冲突）。
enum ThumbSource {
    /// 托管目录里的文件（images/hash.ext）。
    Managed(String),
    /// 待入库图片：缓存 key + 源路径。
    Pending { key: String, path: PathBuf },
}

/// 长按拖动的阶段（§1，ADR-0005）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LongPress {
    /// 计时未达标：还在等待窗口内，尚未决定是点击还是长按。
    Waiting,
    /// 已达标：280ms 一到即进入此态（即使指针尚未移动，也立刻给出虚影反馈）。
    Armed,
    /// 达标前累计位移超过 6pt：本次手势取消，之后按住也不再进入拖动。
    Cancelled,
}

/// 侧边栏长按拖动的进行中手势（§1，替代旧的
/// `drag_row_id` / `drag_moved` / `drag_started` / `drag_target`）。
struct RowDrag {
    /// 被按下的示例 id。
    row_id: i64,
    /// 当前阶段。
    phase: LongPress,
    /// 自指针物理按下起的累计位移长度（pt）。
    displacement: f32,
    /// 达标瞬间的累计位移：用于「达标后再移动 4pt」的判定。
    displacement_at_arm: f32,
    /// 按下点相对行左上角的偏移，虚影因此 1:1 跟手（§2）。
    grab_offset: Vec2,
    /// 行尺寸快照（行高固定 52pt）。
    row_size: Vec2,
    /// 最近一次真实落点下标；指针无坐标时沿用（§1 越界处理）。
    insert: Option<usize>,
    /// 最近一次真实指针位置；指针移出窗口时虚影停在此处（§1）。
    last_pointer: egui::Pos2,
}

impl RowDrag {
    /// 达标后是否已产生足够位移进入「重排中」（§1：4pt）。
    fn moved(&self) -> bool {
        reorder_moved(self.phase, self.displacement, self.displacement_at_arm)
    }
}

/// 松手时的收尾动作（§1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GestureOutcome {
    /// 已取消的手势：不选中、不重排（「按下即快速拖动」的严格语义）。
    None,
    /// 未达标松手：按一次普通点击处理（落在标题上则进入内联编辑）。
    Click,
    /// 达标但未移动、或落点为原位（空操作）：只选中该行。
    Select,
    /// 达标 + 已移动 + 落点改变顺序：重排。
    Reorder,
}

/// 画一整行侧边栏条目：缩略图 + 标题，选中态高亮。
///
/// `rect` 由调用方给定，因此同一个函数既能画列表里的行，也能画 Foreground
/// 图层上的虚影；`opacity` 用于被拖源行的淡占位与虚影（§2），
/// `elevated` 为真时给行底加一层投影（虚影专用）。
#[allow(clippy::too_many_arguments)]
fn paint_sidebar_row(
    ui: &mut egui::Ui,
    rect: Rect,
    selected: bool,
    hovered: bool,
    title: &str,
    thumb: Option<&ThumbSource>,
    images_dir: &Path,
    cache: &mut ImageCache,
    draft: bool,
    editing: bool,
    // 锁图标：None 不画、Some(true) 常显 🔒、Some(false) 悬停显 🔓（§3，ADR-0007）。
    lock: Option<bool>,
    // 来源锁（ADR-0012）：🔒 用强调色，与手动锁区分。
    source_locked: bool,
    opacity: f32,
    elevated: bool,
) {
    let p = theme::Palette::current(ui.ctx());
    let bg = if selected {
        p.row_selected
    } else if hovered {
        p.row_hover
    } else if elevated {
        // 虚影保持该行自身底色：未选中行平时透明，浮起时给一层浮层底色（§2）。
        p.surface
    } else {
        Color32::TRANSPARENT
    };
    // 虚影的投影：epaint 没有真模糊，用偏移 + 半透明形状近似（§2）。
    if elevated {
        ui.painter().add(
            egui::epaint::Shadow {
                offset: [0, 4],
                blur: 12,
                spread: 0,
                color: p.shadow,
            }
            .as_shape(rect, CornerRadius::same(6)),
        );
    }
    ui.painter()
        .rect_filled(rect, CornerRadius::same(6), with_opacity(bg, opacity));
    // 选中态：左侧 3pt 竖条（行高 60%），与底色一起标识当前示例。
    if selected {
        let bar = Rect::from_min_size(
            egui::pos2(rect.left() + 3.0, rect.center().y - rect.height() * 0.3),
            Vec2::new(3.0, rect.height() * 0.6),
        );
        ui.painter().rect_filled(
            bar,
            CornerRadius::same(2),
            with_opacity(p.row_selected_bar, opacity),
        );
    }

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
            let alpha = (255.0 * opacity).round().clamp(0.0, 255.0) as u8;
            egui::Image::new(&cached.texture)
                .fit_to_exact_size(size)
                .tint(Color32::from_white_alpha(alpha))
                .paint_at(ui, target);
            drew_thumb = true;
        }
    }
    if !drew_thumb {
        ui.painter().rect_filled(
            thumb_rect,
            CornerRadius::same(4),
            with_opacity(p.thumb_slot, opacity),
        );
        ui.painter().text(
            thumb_rect.center(),
            egui::Align2::CENTER_CENTER,
            "无图",
            egui::FontId::proportional(11.0),
            with_opacity(p.text_faint, opacity),
        );
    }

    let text_rect = sidebar_row_text_rect(rect);

    // 标题：用 painter 直接画，缩略图与标题因此可以在同一行精确对齐。
    // 内联编辑时标题交给 TextEdit 画，这里跳过。
    if !editing {
        // 换行宽度留出右侧固定图标道（20pt），避免未锁行悬停出现 🔓 时标题重排跳动。
        let galley = ui.painter().layout(
            title.to_string(),
            egui::FontId::proportional(13.5),
            with_opacity(
                if selected {
                    p.text_strong
                } else {
                    p.text
                },
                opacity,
            ),
            (text_rect.width() - 20.0).max(20.0),
        );
        // 垂直居中（§3）：单行标题在 52pt 行高里上下居中。
        ui.painter().galley(
            egui::pos2(
                text_rect.left(),
                rect.center().y - galley.size().y * 0.5,
            ),
            galley,
            Color32::PLACEHOLDER,
        );
    }

    // 锁图标（§3，ADR-0007）：画在右侧固定图标道内、与行上下居中。
    // 锁定时常显 🔒；未锁定时悬停该行才显 🔓，提示可点击上锁。
    if let Some(locked) = lock {
        let icon_rect = sidebar_row_lock_rect(rect);
        ui.painter().text(
            icon_rect.center(),
            egui::Align2::CENTER_CENTER,
            if locked { "🔒" } else { "🔓" },
            egui::FontId::proportional(13.0),
            with_opacity(
                if source_locked { p.accent } else { p.text_muted },
                opacity,
            ),
        );
    }

    // 草稿标记（§8）。
    if draft {
        ui.painter().text(
            egui::pos2(rect.right() - 6.0, rect.bottom() - 5.0),
            egui::Align2::RIGHT_BOTTOM,
            "草稿",
            egui::FontId::proportional(10.0),
            with_opacity(p.text_muted, opacity),
        );
    }
}

/// 固定的「+新示例」行（§4）。
///
/// 与普通示例行同尺寸（高 52pt、宽 `available_width - 8.0`），但不显示缩略图，
/// 文案在行内水平 + 垂直居中（拉宽侧栏后仍居中）；不可拖动、不进落点计算、永不高亮为选中蓝。
/// 返回本帧是否被点击（点击语义沿用 `self.select(None)`）。
fn new_example_row(ui: &mut egui::Ui) -> bool {
    let p = theme::Palette::current(ui.ctx());
    let width = (ui.available_width() - 8.0).max(60.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 52.0), Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().rect_filled(
        rect,
        CornerRadius::same(6),
        if hovered { p.row_hover } else { Color32::TRANSPARENT },
    );
    let color = if hovered { p.text } else { p.text_muted };
    let galley = ui.painter().layout_no_wrap(
        "+新示例".to_string(),
        egui::FontId::proportional(13.5),
        color,
    );
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        color,
    );
    response
        .on_hover_text("新建示例（草稿）：保存时才落库")
        .clicked()
}

/// 侧边栏一行里缩略图所占的子矩形。
fn sidebar_row_thumb_rect(rect: Rect) -> Rect {
    Rect::from_min_size(
        rect.min + Vec2::new(6.0, 6.0),
        Vec2::splat(rect.height() - 12.0),
    )
}

/// 侧边栏一行里标题所占的子矩形；绘制与点击命中判定共用，保证一致（§2）。
/// 锁图标的独占命中区：贴在行右端图标道内、与行同高居中对齐（§3，ADR-0007）。
fn sidebar_row_lock_rect(rect: Rect) -> Rect {
    let text_rect = sidebar_row_text_rect(rect);
    let width = 20.0;
    Rect::from_min_max(
        egui::pos2(text_rect.right() - width, rect.center().y - 26.0),
        egui::pos2(text_rect.right(), rect.center().y + 26.0),
    )
}

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

/// 拖拽落点（§3 越界语义）：指针在列表视口**上方**（含固定「+新示例」行区域）→ 第 0 位；
/// 在视口**下方** → 末尾；否则按键针 y 与所有行的中心线求插入下标。
fn drag_insertion_index(
    pointer_y: f32,
    viewport_top: f32,
    viewport_bottom: f32,
    row_centers: &[f32],
) -> usize {
    if pointer_y < viewport_top {
        0
    } else if pointer_y > viewport_bottom {
        row_centers.len()
    } else {
        insertion_index(pointer_y, row_centers)
    }
}

/// 被拖行在**可重排行**中的下标：与 `row_rects` 对齐（草稿行不在其中）。
fn drag_source_index(rows: &[SidebarRow], row_id: i64) -> Option<usize> {
    rows.iter()
        .filter_map(|row| row.id)
        .position(|id| id == row_id)
}

/// 长按阶段转移（§1，ADR-0005）：从指针物理按下计时，达标前位移超过 6pt 即取消。
///
/// * `phase`：上一帧的阶段；`Cancelled` 与 `Armed` 都是终态。
/// * `elapsed`：从物理按下起算的时长。
/// * `displacement`：自按下起累计位移长度（pt）。
fn long_press_transition(phase: LongPress, elapsed: Duration, displacement: f32) -> LongPress {
    match phase {
        LongPress::Cancelled => LongPress::Cancelled,
        LongPress::Armed => LongPress::Armed,
        LongPress::Waiting => {
            if displacement > REORDER_CANCEL_SLOP {
                // 达标前移动即取消：即使同帧已满时长，也按取消处理。
                LongPress::Cancelled
            } else if elapsed >= REORDER_HOLD {
                LongPress::Armed
            } else {
                LongPress::Waiting
            }
        }
    }
}

/// 达标后是否已产生足够位移进入「重排中」（§1：4pt）。
///
/// `at_arm` 是达标瞬间的累计位移，因此「达标时已经移动了几 pt」不会顶替这 4pt。
fn reorder_moved(phase: LongPress, displacement: f32, at_arm: f32) -> bool {
    phase == LongPress::Armed && displacement - at_arm > REORDER_SLOP
}

/// 松手判定（§1）：返回收尾动作与（重排时）落点下标。
///
/// 「原地松手 = 选中」「空操作不重排」都由这里统一决定；未达标松手仍按一次普通点击处理。
fn gesture_outcome(
    phase: LongPress,
    moved: bool,
    destination_changes: bool,
    insert: Option<usize>,
) -> (GestureOutcome, Option<usize>) {
    match phase {
        LongPress::Cancelled => (GestureOutcome::None, None),
        LongPress::Waiting => (GestureOutcome::Click, None),
        LongPress::Armed => {
            if moved && destination_changes {
                (GestureOutcome::Reorder, insert)
            } else {
                (GestureOutcome::Select, None)
            }
        }
    }
}

/// 拖到列表视口上 / 下边缘时的自动滚动速度（pt/s；正 = 向列表末尾方向滚）。
///
/// 指针在热区内按进入深度线性加速（§3：内缘约 4pt/帧 ≈ 240pt/s、贴边约 16pt/帧 ≈ 960pt/s）；
/// 指针移出视口则保持满速，让长列表里的「拖到最前 / 最后」也能到达。
fn autoscroll_speed(pointer_y: f32, viewport_top: f32, viewport_bottom: f32) -> f32 {
    if pointer_y < viewport_top {
        return -AUTOSCROLL_MAX_SPEED;
    }
    if pointer_y > viewport_bottom {
        return AUTOSCROLL_MAX_SPEED;
    }
    let from_top = pointer_y - viewport_top;
    if from_top < AUTOSCROLL_ZONE {
        let t = 1.0 - from_top / AUTOSCROLL_ZONE;
        return -(AUTOSCROLL_MIN_SPEED + (AUTOSCROLL_MAX_SPEED - AUTOSCROLL_MIN_SPEED) * t);
    }
    let from_bottom = viewport_bottom - pointer_y;
    if from_bottom < AUTOSCROLL_ZONE {
        let t = 1.0 - from_bottom / AUTOSCROLL_ZONE;
        return AUTOSCROLL_MIN_SPEED + (AUTOSCROLL_MAX_SPEED - AUTOSCROLL_MIN_SPEED) * t;
    }
    0.0
}

/// 给颜色乘上不透明度（用于被拖源行的淡占位与虚影，§2）。
fn with_opacity(color: Color32, opacity: f32) -> Color32 {
    if opacity >= 1.0 {
        return color;
    }
    color.gamma_multiply(opacity.clamp(0.0, 1.0))
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

/// 程序图标：调色板蓝底 + 白色相框里的山与太阳（见 src/icon.rs）。
fn app_icon() -> egui::IconData {
    const SIZE: u32 = 64;
    egui::IconData {
        rgba: icon::render(SIZE),
        width: SIZE,
        height: SIZE,
    }
}

/// 标题栏上的文字按钮：常态无底，悬停 / 按下才显形（D1，自绘标题栏）。
fn chrome_button(ui: &mut egui::Ui, text: &str, p: &theme::Palette) -> egui::Response {
    let font = FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font.clone(), p.text);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(galley.size().x + 20.0, 28.0), Sense::click());
    let pressed = response.is_pointer_button_down_on();
    if pressed {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), p.row_selected);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), p.row_hover);
    }
    let color = if pressed { p.text_strong } else { p.text };
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, text, font, color);
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// 下拉菜单项：整行可点，选中项右侧画一个 accent 勾。
fn menu_item(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    p: &theme::Palette,
) -> egui::Response {
    let font = FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), p.text);
    let width = (galley.size().x + 56.0).max(180.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 32.0), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(4), p.row_hover);
    }
    ui.painter().galley(
        egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y * 0.5),
        galley,
        p.text,
    );
    if selected {
        let center = rect.right_center() - Vec2::new(15.0, 0.0);
        let stroke = Stroke::new(1.6, p.accent);
        ui.painter().line_segment(
            [
                center + Vec2::new(-3.5, 0.0),
                center + Vec2::new(-1.0, 3.0),
            ],
            stroke,
        );
        ui.painter().line_segment(
            [
                center + Vec2::new(-1.0, 3.0),
                center + Vec2::new(4.0, -3.5),
            ],
            stroke,
        );
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

/// 导出列表的复选框：18×18 三态自绘（全选 / 部分 / 未选）。
fn export_checkbox(
    painter: &egui::Painter,
    rect: Rect,
    checked: bool,
    partial: bool,
    p: &theme::Palette,
) {
    let corner = CornerRadius::same(4);
    if checked || partial {
        painter.rect_filled(rect, corner, p.accent);
        let stroke = Stroke::new(1.8, p.accent_fg);
        if partial {
            painter.hline(
                rect.left() + 4.5..=rect.right() - 4.5,
                rect.center().y,
                stroke,
            );
        } else {
            let c = rect.center();
            painter.line_segment(
                [c + Vec2::new(-4.5, 0.2), c + Vec2::new(-1.2, 3.6)],
                stroke,
            );
            painter.line_segment(
                [c + Vec2::new(-1.2, 3.6), c + Vec2::new(4.8, -3.4)],
                stroke,
            );
        }
    } else {
        painter.rect_stroke(
            rect,
            corner,
            Stroke::new(1.0, p.border_strong),
            StrokeKind::Inside,
        );
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WindowButton {
    Minimize,
    Maximize,
    Close,
}

/// 窗口按钮：46×40，贴右边缘；关闭键悬停变危险色。
fn window_button(ui: &mut egui::Ui, kind: WindowButton, p: &theme::Palette) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(WINDOW_BUTTON_WIDTH, TITLE_BAR_HEIGHT), Sense::click());
    let hovered = response.hovered();
    let closing = kind == WindowButton::Close;
    if hovered {
        let bg = if closing { p.danger } else { p.row_hover };
        ui.painter().rect_filled(rect, CornerRadius::ZERO, bg);
    }
    let color = if closing && hovered { p.accent_fg } else { p.text };
    let stroke = Stroke::new(1.0, color);
    let c = rect.center();
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
    let painter = ui.painter();
    match kind {
        WindowButton::Minimize => {
            painter.hline(c.x - 5.0..=c.x + 5.0, c.y, stroke);
        }
        WindowButton::Maximize => {
            if maximized {
                let back = Rect::from_center_size(c + Vec2::new(1.5, -1.5), Vec2::splat(7.0));
                let front = Rect::from_center_size(c + Vec2::new(-1.5, 1.5), Vec2::splat(7.0));
                painter.rect_stroke(back, CornerRadius::same(1), stroke, StrokeKind::Inside);
                painter.rect_stroke(front, CornerRadius::same(1), stroke, StrokeKind::Inside);
            } else {
                let box_rect = Rect::from_center_size(c, Vec2::splat(10.0));
                painter.rect_stroke(
                    box_rect,
                    CornerRadius::same(1),
                    stroke,
                    StrokeKind::Inside,
                );
            }
        }
        WindowButton::Close => {
            painter.line_segment(
                [c + Vec2::new(-5.0, -5.0), c + Vec2::new(5.0, 5.0)],
                stroke,
            );
            painter.line_segment(
                [c + Vec2::new(-5.0, 5.0), c + Vec2::new(5.0, -5.0)],
                stroke,
            );
        }
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
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

    /// §3 越界落点语义：视口上方（含固定行区域）→ 第 0 位；视口下方 → 末尾。
    #[test]
    fn drag_insertion_index_clamps_outside_the_viewport() {
        let centers = centers4();
        let (top, bottom) = (10.0, 240.0);
        assert_eq!(drag_insertion_index(100.0, top, bottom, &centers), 2);
        assert_eq!(drag_insertion_index(230.0, top, bottom, &centers), 4);
        assert_eq!(drag_insertion_index(bottom, top, bottom, &centers), 4); // 正好在底边：不算「下方」
        assert_eq!(drag_insertion_index(bottom + 1.0, top, bottom, &centers), 4);
        assert_eq!(drag_insertion_index(9999.0, top, bottom, &centers), 4);
        assert_eq!(drag_insertion_index(top, top, bottom, &centers), 0); // 正好在顶边：按行中心线算
        assert_eq!(drag_insertion_index(top - 1.0, top, bottom, &centers), 0); // 视口上方 → 第 0 位
        assert_eq!(drag_insertion_index(-999.0, top, bottom, &centers), 0);
        // 即使行中心线落在指针上方（列表已滚动），视口上方也一律第 0 位。
        let scrolled = vec![-100.0, -50.0, 30.0, 85.0];
        assert_eq!(insertion_index(0.0, &scrolled), 2);
        assert_eq!(drag_insertion_index(0.0, top, bottom, &scrolled), 0);
        assert_eq!(drag_insertion_index(50.0, top, bottom, &[]), 0); // 空列表
        assert_eq!(drag_insertion_index(9999.0, top, bottom, &[]), 0);
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

    // ------------------------------------------------------------ 0.1.2 手势状态机

    /// §1 边界：达标前累计位移超过 6pt → 取消；之后按住也不再进入拖动。
    #[test]
    fn long_press_cancels_when_moved_before_hold() {
        let before = REORDER_HOLD - Duration::from_millis(1);
        // 恰好等于 6pt：不算「超过」，仍在等待窗口。
        assert_eq!(
            long_press_transition(LongPress::Waiting, before, REORDER_CANCEL_SLOP),
            LongPress::Waiting
        );
        assert_eq!(
            long_press_transition(LongPress::Waiting, before, REORDER_CANCEL_SLOP + 1e-3),
            LongPress::Cancelled
        );
        // 同帧既满时长又超过取消阈值：达标前移动即取消优先。
        assert_eq!(
            long_press_transition(LongPress::Waiting, REORDER_HOLD, REORDER_CANCEL_SLOP + 1e-3),
            LongPress::Cancelled
        );
        // 取消是终态：继续按住、继续移动都不再达标。
        assert_eq!(
            long_press_transition(LongPress::Cancelled, REORDER_HOLD * 10, 0.0),
            LongPress::Cancelled
        );
        assert_eq!(
            long_press_transition(LongPress::Cancelled, REORDER_HOLD * 10, 500.0),
            LongPress::Cancelled
        );
    }

    /// §1：按住不动满 280ms 即达标（即使指针没动，也要立刻给虚影反馈）。
    #[test]
    fn long_press_arms_at_hold_even_without_movement() {
        assert_eq!(
            long_press_transition(LongPress::Waiting, REORDER_HOLD - Duration::from_millis(1), 0.0),
            LongPress::Waiting
        );
        assert_eq!(
            long_press_transition(LongPress::Waiting, REORDER_HOLD, 0.0),
            LongPress::Armed
        );
        // 恰好 6pt 位移 + 满时长：先判定不取消，于是达标。
        assert_eq!(
            long_press_transition(LongPress::Waiting, REORDER_HOLD, REORDER_CANCEL_SLOP),
            LongPress::Armed
        );
        // 达标后保持达标。
        assert_eq!(
            long_press_transition(LongPress::Armed, REORDER_HOLD * 2, 500.0),
            LongPress::Armed
        );
    }

    /// §1 边界：达标后移动超过 4pt 才算进入「重排中」。
    #[test]
    fn reorder_needs_slop_after_arming() {
        assert!(!reorder_moved(LongPress::Armed, REORDER_SLOP, 0.0));
        assert!(reorder_moved(LongPress::Armed, REORDER_SLOP + 1e-3, 0.0));
        // 达标瞬间已经有位移时，从那一刻重新累计 4pt。
        assert!(!reorder_moved(LongPress::Armed, 6.0, 2.0));
        assert!(reorder_moved(LongPress::Armed, 6.0 + 1e-3, 2.0));
        // 未达标 / 已取消都不算「重排中」。
        assert!(!reorder_moved(LongPress::Waiting, 500.0, 0.0));
        assert!(!reorder_moved(LongPress::Cancelled, 500.0, 0.0));
    }

    /// §1：未达标松手 = 点击；达标未移动或空操作 = 选中；只有真正改序才重排。
    #[test]
    fn release_outcomes_follow_the_gesture_phase() {
        assert_eq!(
            gesture_outcome(LongPress::Waiting, false, false, None),
            (GestureOutcome::Click, None)
        );
        assert_eq!(
            gesture_outcome(LongPress::Armed, false, true, Some(3)),
            (GestureOutcome::Select, None)
        );
        assert_eq!(
            gesture_outcome(LongPress::Armed, true, false, Some(1)),
            (GestureOutcome::Select, None)
        );
        assert_eq!(
            gesture_outcome(LongPress::Armed, true, true, Some(2)),
            (GestureOutcome::Reorder, Some(2))
        );
        // 取消的手势：不选中、不重排（「按下即快速拖动」的严格语义）。
        assert_eq!(
            gesture_outcome(LongPress::Cancelled, false, false, None),
            (GestureOutcome::None, None)
        );
        assert_eq!(
            gesture_outcome(LongPress::Cancelled, true, true, Some(2)),
            (GestureOutcome::None, None)
        );
    }

    /// §3：自动滚动方向与热区曲线（视口中部为 0、上热区为负、下热区为正、视口外满速）。
    #[test]
    fn autoscroll_speed_curve_and_direction() {
        let (top, bottom) = (100.0, 400.0);
        assert_eq!(autoscroll_speed((top + bottom) / 2.0, top, bottom), 0.0);
        // 热区内缘（距边缘正好 20pt）：速度归零。
        assert!(autoscroll_speed(top + AUTOSCROLL_ZONE, top, bottom).abs() < 1e-3);
        assert!(autoscroll_speed(bottom - AUTOSCROLL_ZONE, top, bottom).abs() < 1e-3);
        // 上热区：向列表开头方向（负），越深越快。
        assert!(autoscroll_speed(top, top, bottom) < 0.0);
        assert!(
            autoscroll_speed(top + 5.0, top, bottom) < autoscroll_speed(top + 15.0, top, bottom)
        );
        // 下热区：方向相反。
        assert!(autoscroll_speed(bottom, top, bottom) > 0.0);
        assert!(
            autoscroll_speed(bottom - 5.0, top, bottom)
                > autoscroll_speed(bottom - 15.0, top, bottom)
        );
        // 视口外保持满速：长列表里「拖到最前 / 最后」也能到达。
        assert_eq!(autoscroll_speed(top - 50.0, top, bottom), -AUTOSCROLL_MAX_SPEED);
        assert_eq!(autoscroll_speed(bottom + 50.0, top, bottom), AUTOSCROLL_MAX_SPEED);
    }

    /// §3 速度曲线锚点：内缘 ≈ 4pt/帧、贴边 ≈ 16pt/帧（60fps 折算 240–960pt/s）。
    #[test]
    fn autoscroll_speed_matches_the_two_reference_points() {
        let (top, bottom) = (100.0, 400.0);
        let max = autoscroll_speed(bottom, top, bottom);
        assert!((max - 16.0 * 60.0).abs() < 1e-3);
        let inner = autoscroll_speed(bottom - AUTOSCROLL_ZONE + 1e-3, top, bottom);
        assert!((inner - 4.0 * 60.0).abs() < 1.0);
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

    /// §6 边界 ②：同样内容只入库一份（同一张图被多条示例共用时不重复写）。
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

    /// 中央区排版是纯几何：用一条独立的近似断言，避免浮点比较卡在 1e-6 上。
    fn assert_rect(actual: Rect, expected: Rect) {
        for (a, e) in [
            (actual.min.x, expected.min.x),
            (actual.min.y, expected.min.y),
            (actual.max.x, expected.max.x),
            (actual.max.y, expected.max.y),
        ] {
            assert!((a - e).abs() < 0.01, "rect {actual:?} != {expected:?}");
        }
    }

    /// 默认窗口 1180×760 的整窗尺寸。这里只当「一块足够大的可用矩形」用，
    /// 取整数值是为了让期望值是好读的十进制，不代表真实中央区（真值来自 `available_rect_before_wrap`）。
    fn avail_1180x760() -> Rect {
        Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(1180.0, 760.0))
    }

    /// 竖图（宽高比 < 1）：图片占左列满高，正/负向在右列上下排（ADR-0011）。
    ///
    /// 期望值是字面量、不复用上面那组常量：常量被改错时这里必须失败。
    #[test]
    fn tall_image_uses_portrait_layout() {
        let layout = central_layout(avail_1180x760(), Some(0.667));
        assert!(layout.portrait);
        assert_rect(
            layout.image,
            Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(495.6, 760.0)),
        );
        assert_rect(
            layout.positive,
            Rect::from_min_size(egui::pos2(501.6, 0.0), Vec2::new(678.4, 467.48)),
        );
        assert_rect(
            layout.negative,
            Rect::from_min_size(egui::pos2(501.6, 473.48), Vec2::new(678.4, 286.52)),
        );
        // 右列两段首尾相接、不留缝：负向底边必须正好落在可用区底边。
        assert!((layout.negative.bottom() - 760.0).abs() < 0.01);
    }

    /// 横图（宽高比 > 1）：图片在上占满整行，正向在左下、负向在右下（各占一条横带）。
    ///
    /// 期望值是字面量、不复用上面那组常量：常量被改错时这里必须失败。
    #[test]
    fn wide_image_puts_prompts_side_by_side_below_the_image() {
        let layout = central_layout(avail_1180x760(), Some(1.78));
        assert!(!layout.portrait);
        assert_rect(
            layout.image,
            Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(1180.0, 319.2)),
        );
        assert_rect(
            layout.positive,
            Rect::from_min_size(egui::pos2(0.0, 325.2), Vec2::new(727.88, 434.8)),
        );
        assert_rect(
            layout.negative,
            Rect::from_min_size(egui::pos2(733.88, 325.2), Vec2::new(446.12, 434.8)),
        );
        // 左右并排：负向必须在正向右侧、且同高（长边留给了图片）。
        assert!(layout.negative.left() > layout.positive.right());
        assert!((layout.negative.height() - layout.positive.height()).abs() < 0.01);
        // 下方横带的底边必须正好落在可用区底边。
        assert!((layout.negative.bottom() - 760.0).abs() < 0.01);
    }

    /// 正方形与「还没有图片」都走横版：竖版只在真的长边朝下时才有意义。
    #[test]
    fn square_and_missing_image_use_landscape_layout() {
        assert!(!central_layout(avail_1180x760(), Some(1.0)).portrait);
        assert!(!central_layout(avail_1180x760(), None).portrait);
    }

    /// 安全网：窄窗口里竖版会把两段文字压成一条缝，条件不满足就退回横版。
    #[test]
    fn narrow_window_falls_back_to_landscape_layout() {
        let avail = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(500.0, 600.0));
        assert!(500.0 * (1.0 - IMAGE_SHARE) - CENTRAL_GAP < MIN_TEXT_COL_W);
        assert!(!central_layout(avail, Some(0.667)).portrait);
    }
}
