//! 开发工具：把程序化图标写成 docs/images 下的图标图片（icon.png / icon.ico）。
//!
//! 用法：cargo run --bin iconasset
//! 图标几何在 src/icon_render.rs；改完设计稿重跑一次即可刷新文档配图。

#[path = "../icon_render.rs"]
mod icon_render;

fn main() -> anyhow::Result<()> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("docs")
        .join("images");
    std::fs::create_dir_all(&dir)?;

    let size = 256u32;
    let rgba = icon_render::render(size);
    let image = image::RgbaImage::from_raw(size, size, rgba).expect("RGBA 长度应匹配");
    image.save(dir.join("icon.png"))?;

    let ico = icon_render::ico(&[16, 24, 32, 48, 64, 128, 256]);
    std::fs::write(dir.join("icon.ico"), ico)?;

    println!("已写入 {}", dir.display());
    Ok(())
}
