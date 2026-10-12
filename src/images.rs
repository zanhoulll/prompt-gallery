//! 图片解码与纹理缓存。
//!
//! 目标要求低资源占用（ADR-0001），因此不把每张图的 full-res 纹理长期留在显存里：
//! * 侧边栏缩略图：解码后按 `THUMB_MAX` 缩边，LRU 上限 `THUMB_CACHE_CAPACITY`；
//! * 详情区大图：只缓存「当前示例」一张，切成小图时立刻释放。

use std::collections::VecDeque;
use std::path::Path;

use anyhow::{anyhow, Result};
use egui::{ColorImage, Context, TextureHandle, TextureOptions};

/// 侧边栏缩略图最长边（物理像素）。
pub const THUMB_MAX: u32 = 96;
/// 详情区预览的最长边；屏幕再大也不需要更高分辨率。
pub const PREVIEW_MAX: u32 = 1600;
/// 缩略图纹理缓存条数。
pub const THUMB_CACHE_CAPACITY: usize = 128;

/// 缓存里的一个纹理及其像素尺寸。
pub struct CachedTexture {
    pub texture: TextureHandle,
    pub size: [usize; 2],
}

impl CachedTexture {
    pub fn aspect(&self) -> f32 {
        if self.size[1] == 0 {
            1.0
        } else {
            self.size[0] as f32 / self.size[1] as f32
        }
    }
}

/// 按插入顺序淘汰的缩略图缓存。
pub struct ImageCache {
    thumbs: std::collections::HashMap<String, CachedTexture>,
    order: VecDeque<String>,
    preview: Option<(String, CachedTexture)>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            thumbs: std::collections::HashMap::new(),
            order: VecDeque::new(),
            preview: None,
        }
    }

    /// 取（必要时生成）缩略图纹理。
    pub fn thumbnail(
        &mut self,
        ctx: &Context,
        key: &str,
        path: &Path,
    ) -> Result<&CachedTexture> {
        if !self.thumbs.contains_key(key) {
            let image = decode_scaled(path, THUMB_MAX)?;
            let handle = ctx.load_texture(
                format!("thumb:{key}"),
                image.image.clone(),
                TextureOptions::LINEAR,
            );
            self.thumbs.insert(
                key.to_string(),
                CachedTexture {
                    texture: handle,
                    size: image.size,
                },
            );
            self.order.push_back(key.to_string());
            while self.order.len() > THUMB_CACHE_CAPACITY {
                if let Some(oldest) = self.order.pop_front() {
                    self.thumbs.remove(&oldest);
                }
            }
        }
        // 命中后把 key 挪到队尾，使其更晚被淘汰。
        if let Some(pos) = self.order.iter().position(|k| k == key) {
            if let Some(k) = self.order.remove(pos) {
                self.order.push_back(k);
            }
        }
        self.thumbs
            .get(key)
            .ok_or_else(|| anyhow!("缩略图缓存未命中：{key}"))
    }

    /// 取（必要时生成）详情区大图纹理；同一张图重复调用只解码一次。
    pub fn preview(&mut self, ctx: &Context, key: &str, path: &Path) -> Result<&CachedTexture> {
        let need_reload = match &self.preview {
            Some((cached_key, _)) => cached_key != key,
            None => true,
        };
        if need_reload {
            // 先释放上一张，避免两张 full-res 纹理同时驻留。
            self.preview = None;
            let image = decode_scaled(path, PREVIEW_MAX)?;
            let handle = ctx.load_texture(
                format!("preview:{key}"),
                image.image,
                TextureOptions::LINEAR,
            );
            self.preview = Some((
                key.to_string(),
                CachedTexture {
                    texture: handle,
                    size: image.size,
                },
            ));
        }
        self.preview
            .as_ref()
            .map(|(_, cached)| cached)
            .ok_or_else(|| anyhow!("预览缓存未命中：{key}"))
    }

    /// 丢弃当前预览（切换示例、删除示例时调用）。
    pub fn drop_preview(&mut self) {
        self.preview = None;
    }
}

/// 解码结果：已转成 egui 的 `ColorImage`，并保留原始像素尺寸。
pub struct DecodedImage {
    pub image: ColorImage,
    pub size: [usize; 2],
}

/// 解码图片并按最长边 `max_edge` 等比缩小（只缩不放）。
pub fn decode_scaled(path: &Path, max_edge: u32) -> Result<DecodedImage> {
    let decoded = image::ImageReader::open(path)
        .map_err(|err| anyhow!("打开图片失败：{err}"))?
        .with_guessed_format()
        .map_err(|err| anyhow!("识别图片格式失败：{err}"))?
        .decode()
        .map_err(|err| anyhow!("解码图片失败：{err}"))?;

    let scaled = if decoded.width().max(decoded.height()) > max_edge {
        decoded.resize(max_edge, max_edge, image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    let rgba = scaled.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(DecodedImage {
        image: ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
        size,
    })
}

/// 在可用空间内按原始宽高比算出图片的显示尺寸。
pub fn fit_size(aspect: f32, available: egui::Vec2) -> egui::Vec2 {
    let available = egui::vec2(available.x.max(1.0), available.y.max(1.0));
    let scaled = egui::vec2(available.y * aspect, available.y);
    if scaled.x <= available.x {
        scaled
    } else {
        egui::vec2(available.x, available.x / aspect.max(f32::EPSILON))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_size_letterboxes_both_orientations() {
        // 宽图受宽度限制
        let wide = fit_size(2.0, egui::vec2(100.0, 100.0));
        assert_eq!(wide, egui::vec2(100.0, 50.0));
        // 高图受高度限制
        let tall = fit_size(0.5, egui::vec2(100.0, 100.0));
        assert_eq!(tall, egui::vec2(50.0, 100.0));
        // 正方形
        let square = fit_size(1.0, egui::vec2(80.0, 40.0));
        assert_eq!(square, egui::vec2(40.0, 40.0));
    }
}
