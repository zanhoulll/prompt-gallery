//! 主题：调色板、全局 Style、弹窗外壳与标准按钮。
//!
//! 全应用只有这里出现颜色字面量；其余 UI 代码一律从 Palette 取色，
//! 这样深色/浅色两套皮肤不会漏掉某个角落。

use std::path::Path;

use eframe::egui::{
    self, Color32, CornerRadius, CursorIcon, FontId, Id, Margin, Response, RichText, Shadow,
    Stroke, StrokeKind, Ui, Vec2,
};
use serde::{Deserialize, Serialize};

/// 外观设置：跟随系统 / 浅色 / 深色（持久化在 config.json）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn preference(self) -> egui::ThemePreference {
        match self {
            Self::System => egui::ThemePreference::System,
            Self::Light => egui::ThemePreference::Light,
            Self::Dark => egui::ThemePreference::Dark,
        }
    }
}

/// config.json 的内容。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub theme: ThemeChoice,
}

/// 配置文件放在数据目录里，和数据库同级。
pub const CONFIG_FILE: &str = "config.json";

/// 读取配置；文件缺失、损坏或字段不识别时静默回退到默认值。
pub fn load_config(data_dir: &Path) -> AppConfig {
    let path = data_dir.join(CONFIG_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
}

/// 写回配置。父目录不存在时先创建。
pub fn save_config(data_dir: &Path, config: &AppConfig) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join(CONFIG_FILE);
    let text = serde_json::to_string_pretty(config)?;
    std::fs::write(&path, text)?;
    Ok(())
}

/// 一套颜色。字段语义见 docs/ui-layout-map.md 与 ADR-0009。
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub canvas: Color32,
    pub surface: Color32,
    pub surface_sunken: Color32,
    pub surface_raised: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_strong: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub accent_fg: Color32,
    pub row_hover: Color32,
    pub row_selected: Color32,
    pub row_selected_bar: Color32,
    pub thumb_slot: Color32,
    pub danger: Color32,
    pub shadow: Color32,
    pub backdrop: Color32,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            canvas: Color32::from_rgb(0x16, 0x18, 0x1C),
            surface: Color32::from_rgb(0x1E, 0x21, 0x26),
            surface_sunken: Color32::from_rgb(0x1C, 0x1E, 0x22),
            surface_raised: Color32::from_rgb(0x1A, 0x1C, 0x20),
            border: Color32::from_rgb(0x48, 0x48, 0x4A),
            border_strong: Color32::from_rgb(0x6E, 0x6E, 0x70),
            text: Color32::from_rgb(0xDC, 0xDC, 0xDC),
            text_strong: Color32::from_rgb(0xEE, 0xEE, 0xEE),
            text_muted: Color32::from_rgb(0x96, 0x96, 0x96),
            text_faint: Color32::from_rgb(0x9A, 0xA0, 0xA6),
            accent: Color32::from_rgb(0x78, 0xAA, 0xFF),
            accent_fg: Color32::from_rgb(0x0E, 0x11, 0x16),
            row_hover: Color32::from_rgb(0x2E, 0x2E, 0x2E),
            row_selected: Color32::from_rgb(0x30, 0x3E, 0x58),
            row_selected_bar: Color32::from_rgb(0x78, 0xAA, 0xFF),
            thumb_slot: Color32::from_rgb(0x3A, 0x3A, 0x3A),
            danger: Color32::from_rgb(0xF0, 0x64, 0x5A),
            shadow: Color32::from_rgba_unmultiplied(0, 0, 0, 115),
            backdrop: Color32::from_rgba_unmultiplied(0, 0, 0, 115),
        }
    }

    pub fn light() -> Self {
        Self {
            canvas: Color32::from_rgb(0xF7, 0xF8, 0xFA),
            surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            surface_sunken: Color32::from_rgb(0xF1, 0xF3, 0xF5),
            surface_raised: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            border: Color32::from_rgb(0xE4, 0xE7, 0xEC),
            border_strong: Color32::from_rgb(0xD0, 0xD5, 0xDD),
            text: Color32::from_rgb(0x1F, 0x23, 0x28),
            text_strong: Color32::from_rgb(0x10, 0x18, 0x28),
            text_muted: Color32::from_rgb(0x66, 0x70, 0x85),
            text_faint: Color32::from_rgb(0x98, 0xA2, 0xB3),
            accent: Color32::from_rgb(0x25, 0x63, 0xEB),
            accent_fg: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            row_hover: Color32::from_rgb(0xED, 0xEF, 0xF2),
            row_selected: Color32::from_rgb(0xE8, 0xEF, 0xFD),
            row_selected_bar: Color32::from_rgb(0x25, 0x63, 0xEB),
            thumb_slot: Color32::from_rgb(0xE9, 0xEC, 0xEF),
            danger: Color32::from_rgb(0xD9, 0x2D, 0x20),
            shadow: Color32::from_rgba_unmultiplied(16, 24, 40, 31),
            backdrop: Color32::from_rgba_unmultiplied(16, 24, 40, 71),
        }
    }

    /// 当前生效皮肤由 egui 的解析结果决定（System 会跟随系统）。
    pub fn current(ctx: &egui::Context) -> Self {
        if ctx.theme() == egui::Theme::Dark {
            Self::dark()
        } else {
            Self::light()
        }
    }
}

/// 把调色板写进全局 Style。
///
/// 每帧只在主题真的变化时调用一次（见 GalleryView::ui）；
/// 主题切换时由「外观」菜单立即调用。
pub fn apply(ctx: &egui::Context) {
    let p = Palette::current(ctx);
    let theme = ctx.theme();
    let dark = theme == egui::Theme::Dark;
    ctx.style_mut_of(theme, |style| {
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 4.0);
        style.spacing.interact_size = Vec2::new(0.0, 28.0);
        style.spacing.menu_margin = Margin::same(6);

        let visuals = &mut style.visuals;
        visuals.dark_mode = dark;
        visuals.panel_fill = p.canvas;
        visuals.window_fill = p.surface;
        visuals.window_stroke = Stroke::new(1.0, p.border);
        visuals.window_corner_radius = CornerRadius::same(12);
        visuals.window_shadow = Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: p.shadow,
        };
        visuals.menu_corner_radius = CornerRadius::same(8);
        visuals.popup_shadow = Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: p.shadow,
        };
        visuals.extreme_bg_color = p.surface_sunken;
        visuals.text_edit_bg_color = None;
        visuals.code_bg_color = p.surface_sunken;
        visuals.faint_bg_color = p.row_hover;
        visuals.hyperlink_color = p.accent;
        visuals.error_fg_color = p.danger;
        visuals.warn_fg_color = p.danger;
        visuals.selection.bg_fill = p.accent.gamma_multiply(0.35);
        visuals.selection.stroke = Stroke::NONE;
        visuals.text_cursor.stroke = Stroke::new(1.5, p.accent);
        visuals.interact_cursor = Some(CursorIcon::PointingHand);
        visuals.image_loading_spinners = false;

        let corner = CornerRadius::same(6);
        visuals.widgets.noninteractive = egui::style::WidgetVisuals {
            bg_fill: p.surface,
            weak_bg_fill: p.surface,
            bg_stroke: Stroke::new(1.0, p.border),
            corner_radius: corner,
            fg_stroke: Stroke::new(1.0, p.text),
            expansion: 0.0,
        };
        visuals.widgets.inactive = egui::style::WidgetVisuals {
            bg_fill: p.surface_raised,
            weak_bg_fill: p.surface_raised,
            bg_stroke: Stroke::new(1.0, p.border),
            corner_radius: corner,
            fg_stroke: Stroke::new(1.0, p.text),
            expansion: 0.0,
        };
        visuals.widgets.hovered = egui::style::WidgetVisuals {
            bg_fill: p.row_hover,
            weak_bg_fill: p.row_hover,
            bg_stroke: Stroke::new(1.0, p.border_strong),
            corner_radius: corner,
            fg_stroke: Stroke::new(1.0, p.text_strong),
            expansion: 0.0,
        };
        visuals.widgets.active = egui::style::WidgetVisuals {
            bg_fill: p.row_selected,
            weak_bg_fill: p.row_selected,
            bg_stroke: Stroke::new(1.0, p.accent),
            corner_radius: corner,
            fg_stroke: Stroke::new(1.0, p.text_strong),
            expansion: 0.0,
        };
        visuals.widgets.open = visuals.widgets.hovered;
    });
}

/// 窗口最外层：圆角画布 + 1pt 描边，给自绘标题栏做底（ADR-0009）。
pub fn window_frame(ctx: &egui::Context, maximized: bool) -> egui::Frame {
    let p = Palette::current(ctx);
    let radius = if maximized {
        CornerRadius::ZERO
    } else {
        CornerRadius::same(10)
    };
    egui::Frame::new()
        .fill(p.canvas)
        .corner_radius(radius)
        .stroke(Stroke::new(1.0, p.border))
        .inner_margin(Margin::same(1))
}

/// 弹窗外壳：统一的圆角、描边、阴影与半透明遮罩。
///
/// 五个弹窗（草稿/关窗/关于/删除/导出）都必须经过这里，避免尺寸与留白漂移。
pub fn modal_shell<R>(
    ctx: &egui::Context,
    id: &str,
    width: f32,
    body: impl FnOnce(&mut Ui) -> R,
) -> R {
    let p = Palette::current(ctx);
    let frame = egui::Frame::new()
        .fill(p.surface)
        .corner_radius(CornerRadius::same(12))
        .stroke(Stroke::new(1.0, p.border))
        .inner_margin(Margin::symmetric(20, 18))
        .shadow(Shadow {
            offset: [0, 8],
            blur: 24,
            spread: 0,
            color: p.shadow,
        });
    egui::Modal::new(Id::new(id))
        .frame(frame)
        .backdrop_color(p.backdrop)
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
            body(ui)
        })
        .inner
}

/// 弹窗标题（16pt 加粗），已带标题下方的 6pt 留白。
pub fn modal_title(ui: &mut Ui, text: &str) {
    let p = Palette::current(ui.ctx());
    ui.label(RichText::new(text).size(16.0).strong().color(p.text_strong));
    ui.add_space(6.0);
}

/// 弹窗说明文字（12pt 次要色），行间 2pt。
pub fn modal_desc(ui: &mut Ui, text: impl Into<String>) {
    let p = Palette::current(ui.ctx());
    ui.label(RichText::new(text.into()).size(12.0).color(p.text_muted));
    ui.add_space(2.0);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ButtonKind {
    Primary,
    Secondary,
    Danger,
}

fn text_button(ui: &mut Ui, text: &str, kind: ButtonKind, enabled: bool) -> Response {
    let p = Palette::current(ui.ctx());
    let font = FontId::proportional(13.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, p.text);
    let size = Vec2::new(galley.size().x + 28.0, 30.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let response = if enabled {
        response.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        response
    };
    let hovered = enabled && response.hovered();

    let (fill, stroke, text_color) = match kind {
        ButtonKind::Primary => {
            let fill = if !enabled {
                p.accent.gamma_multiply(0.35)
            } else if hovered {
                p.accent.gamma_multiply(1.10)
            } else {
                p.accent
            };
            let text_color = if enabled { p.accent_fg } else { p.text_muted };
            (fill, Stroke::NONE, text_color)
        }
        ButtonKind::Secondary => {
            let fill = if hovered {
                p.row_hover
            } else {
                Color32::TRANSPARENT
            };
            (
                fill,
                Stroke::new(1.0, p.border),
                if enabled { p.text } else { p.text_faint },
            )
        }
        ButtonKind::Danger => {
            let fill = if !enabled {
                Color32::TRANSPARENT
            } else if hovered {
                p.danger.gamma_multiply(0.12)
            } else {
                Color32::TRANSPARENT
            };
            (
                fill,
                Stroke::new(1.0, if enabled { p.danger } else { p.border_strong }),
                if enabled { p.danger } else { p.text_faint },
            )
        }
    };

    ui.painter()
        .rect(rect, CornerRadius::same(6), fill, stroke, StrokeKind::Inside);
    let text_pos = rect.center() - galley.size() * 0.5;
    ui.painter().galley(text_pos, galley, text_color);
    response
}

/// 主操作：填充强调色。
pub fn primary_button(ui: &mut Ui, text: &str) -> Response {
    text_button(ui, text, ButtonKind::Primary, true)
}

/// 主操作，可控启用状态（置灰时不可点）。
pub fn primary_button_enabled(ui: &mut Ui, text: &str, enabled: bool) -> Response {
    text_button(ui, text, ButtonKind::Primary, enabled)
}

/// 次操作：透明底 + 描边。
pub fn secondary_button(ui: &mut Ui, text: &str) -> Response {
    text_button(ui, text, ButtonKind::Secondary, true)
}

/// 危险操作：透明底 + 危险色描边与文字。
pub fn danger_button(ui: &mut Ui, text: &str) -> Response {
    text_button(ui, text, ButtonKind::Danger, true)
}
