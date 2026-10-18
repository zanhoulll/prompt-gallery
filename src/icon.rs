//! 应用图标：程序化自绘（ADR-0009 D3），像素生成在 icon_render.rs。
//!
//! 这里只做 egui 侧的事：把图标画到标题栏左上角（18×18，纹理按像素尺寸缓存）。
//! exe 资源图标由 build.rs 用同一份 render/ico 生成（ADR-0010）。

// ico() 只给 build.rs 与 iconasset 用，GUI 侧只用 render()。
#[allow(dead_code)]
#[path = "icon_render.rs"]
mod icon_render;

pub use icon_render::render;

use eframe::egui::{self, Id, Rect, TextureHandle, TextureOptions};

/// 在给定矩形里绘制图标（标题栏 18×18）。纹理按像素尺寸缓存。
pub fn paint(painter: &egui::Painter, rect: Rect) {
    let px = ((rect.width().max(1.0) * 4.0).round() as u32).clamp(16, 256);
    let ctx = painter.ctx();
    let key = Id::new(("app_icon_texture", px));
    let cached = ctx.data(|data| data.get_temp::<TextureHandle>(key));
    let texture = match cached {
        Some(handle) => handle,
        None => {
            let rgba = render(px);
            let image =
                egui::ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &rgba);
            let handle = ctx.load_texture("app-icon", image, TextureOptions::LINEAR);
            ctx.data_mut(|data| data.insert_temp(key, handle.clone()));
            handle
        }
    };
    painter.image(
        texture.id(),
        rect,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
}
