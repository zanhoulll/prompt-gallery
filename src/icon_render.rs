//! 图标像素生成：纯计算，无外部依赖。
//!
//! 256×256 设计稿（ADR-0009 D3），按目标尺寸超采样后盒式降采样。
//! 同一套几何供四处使用：窗口图标（icon::render）、标题栏（icon::paint）、
//! exe 资源图标（build.rs）、文档配图（src/bin/iconasset.rs）。

/// 图标设计尺寸。
const DESIGN: f32 = 256.0;

fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// 圆角矩形的有符号距离：小于 0 在内部。
fn rounded_rect_sdf(x: f32, y: f32, cx: f32, cy: f32, hx: f32, hy: f32, radius: f32) -> f32 {
    let qx = (x - cx).abs() - (hx - radius);
    let qy = (y - cy).abs() - (hy - radius);
    let ox = qx.max(0.0);
    let oy = qy.max(0.0);
    (ox * ox + oy * oy).sqrt() + qx.max(qy).min(0.0) - radius
}

/// 三角形内部判定（顶点按顺时针或逆时针都行）。
fn in_triangle(x: f32, y: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let edge = |p: (f32, f32), q: (f32, f32)| (q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0);
    let d1 = edge(a, b);
    let d2 = edge(b, c);
    let d3 = edge(c, a);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

/// 设计稿上某点的颜色；None 表示透明（圆角之外）。
fn sample(x: f32, y: f32) -> Option<(u8, u8, u8)> {
    // 1) 底板：竖向渐变 + 大圆角。
    if rounded_rect_sdf(x, y, 128.0, 128.0, 128.0, 128.0, 56.0) > 0.0 {
        return None;
    }
    let mut color = lerp((0x4F, 0x8E, 0xF7), (0x2F, 0x6F, 0xD0), y / DESIGN);

    // 2) 相框：白色描边宽 16，内缩 40。
    let outer = rounded_rect_sdf(x, y, 128.0, 128.0, 96.0, 96.0, 12.0);
    let inner = rounded_rect_sdf(x, y, 128.0, 128.0, 80.0, 80.0, 2.0);
    if outer <= 0.0 && inner > 0.0 {
        color = (0xFF, 0xFF, 0xFF);
    }

    // 3) 相框内部：天空、太阳、两座山。
    if inner <= 0.0 {
        color = (0xBF, 0xE0, 0xFF);

        let dx = x - 176.0;
        let dy = y - 96.0;
        if dx * dx + dy * dy <= 18.0 * 18.0 {
            color = (0xFF, 0xD1, 0x66);
        }

        // 远山（浅绿）在后。
        if in_triangle(x, y, (56.0, 208.0), (184.0, 208.0), (96.0, 120.0)) {
            color = (0x6F, 0xBF, 0x73);
        }
        // 近山（深绿）在前。
        if in_triangle(x, y, (88.0, 208.0), (208.0, 208.0), (144.0, 150.0)) {
            color = (0x4E, 0x9A, 0x54);
        }
    }

    Some(color)
}

/// 生成 RGBA 位图（直通 alpha），长度恒为 size × size × 4。
pub fn render(size: u32) -> Vec<u8> {
    let size = size.max(1);
    let ss: u32 = if size >= 48 { 4 } else { 8 };
    let dim = size * ss;
    let mut hi = vec![0u8; (dim * dim * 4) as usize];
    for y in 0..dim {
        for x in 0..dim {
            let ux = (x as f32 + 0.5) / dim as f32 * DESIGN;
            let uy = (y as f32 + 0.5) / dim as f32 * DESIGN;
            if let Some((r, g, b)) = sample(ux, uy) {
                let index = ((y * dim + x) * 4) as usize;
                hi[index] = r;
                hi[index + 1] = g;
                hi[index + 2] = b;
                hi[index + 3] = 255;
            }
        }
    }
    if ss == 1 {
        return hi;
    }

    // 盒式降采样：每个输出像素取 ss×ss 个超采样像素的平均值。
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..ss {
                for sx in 0..ss {
                    let index = (((y * ss + sy) * dim + (x * ss + sx)) * 4) as usize;
                    r += hi[index] as u32;
                    g += hi[index + 1] as u32;
                    b += hi[index + 2] as u32;
                    a += hi[index + 3] as u32;
                }
            }
            let count = (ss * ss) as u32;
            let index = ((y * size + x) * 4) as usize;
            out[index] = (r / count) as u8;
            out[index + 1] = (g / count) as u8;
            out[index + 2] = (b / count) as u8;
            out[index + 3] = (a / count) as u8;
        }
    }
    out
}

/// 把给定尺寸打包成 Windows .ico（每个尺寸一条 32bpp BMP 条目）。
///
/// exe 内嵌图标只认 .ico（ADR-0010）；用 BMP 条目而不是内嵌 PNG，
/// 是因为小尺寸走 PNG 解码器反而更慢，且 BMP 条目实现最简单。
pub fn ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&size| dib(size)).collect();
    let mut out = Vec::new();
    // ICONDIR
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    // ICONDIRENTRY ×n
    let mut offset = 6 + 16 * images.len() as u32;
    for (size, data) in sizes.iter().zip(&images) {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        out.push(dim);
        out.push(dim);
        out.push(0);
        out.push(0);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for data in &images {
        out.extend_from_slice(data);
    }
    out
}

/// 单个尺寸的 ICO 图像数据：BITMAPINFOHEADER + 自底向上 BGRA + 全零 AND 掩码。
fn dib(size: u32) -> Vec<u8> {
    let size = size.max(1);
    let rgba = render(size);
    let pixel_bytes = (size * size * 4) as usize;
    let mask_stride = ((size as usize + 31) / 32) * 4;

    let mut out = Vec::with_capacity(40 + pixel_bytes + mask_stride * size as usize);
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(size as i32).to_le_bytes()); // biWidth
    out.extend_from_slice(&((size as i32) * 2).to_le_bytes()); // biHeight = XOR + AND
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes()); // biSizeImage
    out.extend_from_slice(&[0u8; 16]); // 分辨率与色表

    for y in (0..size).rev() {
        for x in 0..size {
            let index = ((y * size + x) * 4) as usize;
            out.push(rgba[index + 2]);
            out.push(rgba[index + 1]);
            out.push(rgba[index]);
            out.push(rgba[index + 3]);
        }
    }

    // AND 掩码全零：透明度由 alpha 通道决定。
    out.resize(out.len() + mask_stride * size as usize, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::{ico, render};

    #[test]
    fn buffer_has_rgba_length() {
        for size in [64, 48, 32, 16] {
            assert_eq!(render(size).len(), (size * size * 4) as usize, "size {size}");
        }
    }

    #[test]
    fn center_is_opaque() {
        for size in [64, 48, 32, 16] {
            let rgba = render(size);
            let index = (((size / 2) * size + size / 2) * 4) as usize;
            assert_eq!(rgba[index + 3], 255, "size {size}");
        }
    }

    #[test]
    fn corners_are_transparent() {
        for size in [64, 48, 32, 16] {
            let rgba = render(size);
            for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
                let index = ((y * size + x) * 4) as usize;
                assert_eq!(rgba[index + 3], 0, "size {size} at ({x},{y})");
            }
        }
    }

    #[test]
    fn small_icon_still_has_the_white_frame() {
        let size = 16;
        let rgba = render(size);
        let mut found = false;
        for pixel in rgba.chunks_exact(4) {
            if pixel[0] > 230 && pixel[1] > 230 && pixel[2] > 230 && pixel[3] == 255 {
                found = true;
                break;
            }
        }
        assert!(found, "16px 图标应保留相框的白色像素");
    }

    #[test]
    fn ico_header_lists_every_size() {
        let sizes = [16u32, 32, 256];
        let bytes = ico(&sizes);
        assert_eq!(u16::from_le_bytes([bytes[0], bytes[1]]), 0, "reserved");
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 1, "type = icon");
        assert_eq!(
            u16::from_le_bytes([bytes[4], bytes[5]]) as usize,
            sizes.len(),
            "条目数"
        );
        for (index, size) in sizes.iter().enumerate() {
            let entry = 6 + index * 16;
            let dim = if *size >= 256 { 0 } else { *size as u8 };
            assert_eq!(bytes[entry], dim, "宽 {size}");
            assert_eq!(bytes[entry + 1], dim, "高 {size}");
            let len = u32::from_le_bytes(bytes[entry + 8..entry + 12].try_into().unwrap());
            let offset = u32::from_le_bytes(bytes[entry + 12..entry + 16].try_into().unwrap());
            assert_eq!(
                len as usize,
                40 + (*size * *size * 4) as usize + mask(*size),
                "长度 {size}"
            );
            assert!(
                offset as usize + len as usize <= bytes.len(),
                "偏移越界 {size}"
            );
        }
    }

    fn mask(size: u32) -> usize {
        (((size as usize + 31) / 32) * 4) * size as usize
    }
}
