//! 交付验收：从零建库 → 导出 zip → 校验 zip 内容（结构、图片去重、分区标题不入正文）。
//!
//! 这是端到端的归档契约测试：`data.json` 的字段名、`images/` 条目命名、以及
//! 「导出正文不含分区标题」这条来自 `CONTEXT.md` 的约束都在这里锁死。

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use prompt_gallery::archive;
use prompt_gallery::model::Example;
use prompt_gallery::store::Store;

/// 测试用临时目录，`Drop` 时自动删除——即使断言失败也会清理，不留残留。
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "prompt-gallery-acceptance-{tag}-{}-{seq}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 1x1 红色 PNG。
const PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D', b'R',
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x50, 0x00, 0x00, 0x00, 0x0A, b'I', b'D', b'A', b'T', 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0,
    0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, b'I', b'E',
    b'N', b'D', 0xAE, 0x42, 0x60, 0x82,
];

#[test]
fn acceptance_export_package_shape() {
    let temp = TempDir::new("export");
    let dir = temp.path().to_path_buf();
    let mut store = Store::open_in(dir.clone()).unwrap();

    // 两条示例共用一张图（验证内容寻址去重：托管目录只应有一个图片文件）。
    let (hash, ext) = store.store_image_bytes(PNG, "png").unwrap();
    let mut first = Example::draft(0);
    first.title = "教室里的银发少女".into();
    first.image_hash = Some(hash.clone());
    first.image_ext = Some(ext.clone());
    first.sections.get_mut(0).unwrap().push_str("masterpiece, best quality");
    first.sections.get_mut(4).unwrap().push_str("classroom, rim light");
    store.insert(&mut first).unwrap();

    let mut second = Example::draft(0);
    second.title = "复用同一张图".into();
    second.image_hash = Some(hash.clone());
    second.image_ext = Some(ext.clone());
    second.sections.get_mut(1).unwrap().push_str("1girl, solo");
    store.insert(&mut second).unwrap();

    let image_files: Vec<String> = fs::read_dir(store.images_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(image_files.len(), 1, "同一张图只应占一个文件：{image_files:?}");
    assert_eq!(image_files[0], format!("{hash}.{ext}"));

    // 导出。
    let zip_path = dir.join(archive::default_file_name(1_704_164_645));
    let count = archive::export(&store, &zip_path).unwrap();
    assert_eq!(count, 2);
    assert_eq!(
        zip_path.file_name().unwrap().to_string_lossy(),
        "prompt-gallery-20240102-030405.zip"
    );
    assert!(!zip_path.with_extension("zip.part").exists(), ".part 临时文件应已改名");

    // 校验 zip 结构。
    let file = fs::File::open(&zip_path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let names: Vec<String> = (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().unwrap().to_string())
        .collect();
    eprintln!("zip entries = {names:#?}");
    assert!(names.iter().any(|n| n == "data.json"));
    assert!(names.iter().any(|n| n == "manifest.txt"));
    assert!(names.iter().any(|n| n == &format!("images/{hash}.{ext}")));
    assert_eq!(
        names.iter().filter(|n| n.starts_with("images/")).count(),
        1,
        "两个示例共用一张图，包里只应有一个图片条目"
    );

    let mut data = String::new();
    zip.by_name("data.json").unwrap().read_to_string(&mut data).unwrap();
    let json: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(json["archive_version"], 1);
    assert_eq!(json["examples"].as_array().unwrap().len(), 2);
    assert_eq!(json["examples"][0]["title"], "教室里的银发少女");
    assert_eq!(json["examples"][0]["image"], format!("{hash}.{ext}"));
    assert_eq!(json["examples"][0]["sections"].as_array().unwrap().len(), 5);
    assert_eq!(json["examples"][0]["sections"][0], "masterpiece, best quality");
    assert_eq!(json["examples"][0]["sections"][2], "");

    // 分区标题绝不能出现在归档正文里。
    for title in prompt_gallery::model::SECTION_TITLES {
        assert!(!data.contains(title), "data.json 不应包含分区标题「{title}」");
    }
}

/// 两条示例共用一张图，返回 (store, 目录守卫, 按库内顺序的 id)。
fn store_with_two_examples(tag: &str) -> (Store, TempDir, Vec<i64>) {
    let temp = TempDir::new(tag);
    let mut store = Store::open_in(temp.path().to_path_buf()).unwrap();
    let (hash, ext) = store.store_image_bytes(PNG, "png").unwrap();
    let mut first = Example::draft(0);
    first.title = "第一条".into();
    first.image_hash = Some(hash.clone());
    first.image_ext = Some(ext.clone());
    let first_id = store.insert(&mut first).unwrap();
    let mut second = Example::draft(0);
    second.title = "第二条".into();
    second.image_hash = Some(hash);
    second.image_ext = Some(ext);
    let second_id = store.insert(&mut second).unwrap();
    (store, temp, vec![first_id, second_id])
}

fn read_entry(zip: &mut zip::ZipArchive<fs::File>, name: &str) -> String {
    let mut text = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut text).unwrap();
    text
}

#[test]
fn export_ids_writes_only_the_chosen_example() {
    let (store, temp, ids) = store_with_two_examples("export-selected");
    let zip_path = temp.path().join("selected.zip");
    let count = archive::export_ids(&store, &ids[..1], &zip_path).unwrap();
    assert_eq!(count, 1);
    let mut zip = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
    let json: serde_json::Value = serde_json::from_str(&read_entry(&mut zip, "data.json")).unwrap();
    let examples = json["examples"].as_array().unwrap();
    assert_eq!(examples.len(), 1, "只应导出一条示例");
    assert_eq!(examples[0]["id"], ids[0]);
    assert_eq!(examples[0]["title"], "第一条");
}

#[test]
fn export_ids_keeps_image_dedup_across_the_selection() {
    let (store, temp, ids) = store_with_two_examples("export-shared-image");
    let zip_path = temp.path().join("both.zip");
    let count = archive::export_ids(&store, &ids, &zip_path).unwrap();
    assert_eq!(count, 2);
    let mut zip = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
    let names: Vec<String> = (0..zip.len())
        .map(|i| zip.by_index(i).unwrap().name().unwrap().to_string())
        .collect();
    assert_eq!(
        names.iter().filter(|n| n.starts_with("images/")).count(),
        1,
        "同图两条示例只应写一个图片条目：{names:?}"
    );
}

#[test]
fn export_ids_rejects_an_empty_selection() {
    let (store, temp, _ids) = store_with_two_examples("export-empty");
    let zip_path = temp.path().join("empty.zip");
    let err = archive::export_ids(&store, &[], &zip_path).unwrap_err();
    assert!(err.to_string().contains("没有选择"), "错误文案应说明没有勾选：{err}");
    assert!(!zip_path.exists(), "失败时不应留下半成品");
}

