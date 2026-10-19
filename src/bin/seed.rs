//! 开发用冒烟数据生成器：`cargo run --bin seed -- <数据目录> <图片路径>`。
//!
//! 它走的是**应用自己的** `store::Store::store_image_bytes`，因此生成的图片
//! 哈希命名与真实使用完全一致——UI 冒烟测试因此能真正验证
//! 「按内容哈希找图并渲染」这条链路，而不是造一份假的数据库。

use std::path::PathBuf;

use prompt_gallery::model::Example;
use prompt_gallery::store::{self, Store};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let data_dir = PathBuf::from(args.next().unwrap_or_else(|| "data".into()));
    let image_path = args.next().map(PathBuf::from);

    let mut store = Store::open_in(data_dir)?;

    let image = match &image_path {
        Some(path) => {
            let bytes = std::fs::read(path)?;
            let ext = store::ext_for_bytes(&bytes).unwrap_or("png");
            let (hash, ext) = store.store_image_bytes(&bytes, ext)?;
            println!("图片已入库：{hash}.{ext}");
            Some((hash, ext))
        }
        None => None,
    };

    // 0.2.0 起只有正/负向两段（ADR-0011）：原来五段在这里合成一段正向。
    let positive = [
        "masterpiece, best quality, very aesthetic, absurdres",
        "1girl, solo, silver hair, blue eyes, detailed face",
        "white shirt, black ribbon tie, pleated skirt",
        "standing, hands behind back, looking at viewer",
        "classroom, window light, rim light, soft shadows",
    ]
    .join("\n\n");

    let mut example = Example::draft(0);
    example.title = "教室里的银发少女".to_string();
    example.positive = positive;
    example.negative = "lowres, worst quality, bad anatomy".to_string();
    if let Some((hash, ext)) = image {
        example.image_hash = Some(hash);
        example.image_ext = Some(ext);
    }
    let first = store.insert(&mut example)?;

    let mut second = Example::draft(0);
    second.title = "无图示例".to_string();
    second.positive = "masterpiece, best quality".to_string();
    let second_id = store.insert(&mut second)?;

    println!(
        "已写入示例 id={first} 与 id={second_id}，共 {} 条",
        store.row_count()
    );
    Ok(())
}
