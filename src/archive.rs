//! 导出 / 导入：把示例打包成 `prompt-gallery-<时间戳>.zip`。
//!
//! 依据 ADR-0003：JSON 里只放元数据与五个分区文本，图片作为独立文件条目进 zip，
//! 因此导出包不会因为大图而膨胀，且图片天然按内容哈希去重。
//!
//! 分区标题是 UI 常量，不写进 JSON 的正文里；只在 `manifest.txt` 里作为说明列出，
//! 保证「复制/导出的正向提示词不含分区标题」这条约束在数据层也成立。

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::model::{compact_timestamp, now_unix, Example, Sections, SECTION_COUNT, SECTION_TITLES};
use crate::store::{ext_for_bytes, Store};

/// 归档 JSON 的 schema 版本。
pub const ARCHIVE_VERSION: u32 = 1;
/// 导出文件名前缀。
pub const ARCHIVE_PREFIX: &str = "prompt-gallery";
/// zip 内的元数据条目名。
const DATA_ENTRY: &str = "data.json";
/// zip 内的说明条目名。
const MANIFEST_ENTRY: &str = "manifest.txt";
/// zip 内图片所在的前缀。
const IMAGES_PREFIX: &str = "images/";

#[derive(Serialize, Deserialize)]
struct ArchiveExample {
    id: i64,
    position: i64,
    title: String,
    /// 图片文件名（形如 `<hash>.<ext>`），对应 zip 内的 `images/<文件名>`；无图片为 `null`。
    image: Option<String>,
    /// 只含五个分区的正文，不含分区标题。
    sections: Vec<String>,
    /// 只读锁（ADR-0007）。旧归档不含该字段时按未锁处理。
    #[serde(default)]
    locked: bool,
    created_at: i64,
    updated_at: i64,
}

#[derive(Serialize, Deserialize)]
struct ArchiveData {
    archive_version: u32,
    app: String,
    exported_at: i64,
    examples: Vec<ArchiveExample>,
}

/// 导入模式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImportMode {
    /// 全部作为新示例追加（默认，任何情况下都不会覆盖已有数据）。
    Append,
    /// 尽量沿用归档里的 id，遇到冲突的记录才另分配新 id。
    KeepIds,
}

/// 导入结果，用于 UI 反馈。
#[derive(Debug, Default)]
pub struct ImportReport {
    pub added: usize,
    pub remapped_ids: usize,
    pub images_written: usize,
    pub missing_images: usize,
    pub skipped: usize,
}

/// 导出包的默认文件名：`prompt-gallery-<时间戳>.zip`（CONTEXT.md）。
pub fn default_file_name(at: i64) -> String {
    format!("{ARCHIVE_PREFIX}-{}.zip", compact_timestamp(at))
}

/// 把图库导出到 `target`。
///
/// 先写 `<target>.part` 再改名，避免半截文件被误当成有效备份。
pub fn export(store: &Store, target: &Path) -> Result<usize> {
    let ids: Vec<i64> = store.rows().iter().map(|row| row.id).collect();

    let mut data = ArchiveData {
        archive_version: ARCHIVE_VERSION,
        app: format!("Prompt Gallery {}", env!("CARGO_PKG_VERSION")),
        exported_at: now_unix(),
        examples: Vec::with_capacity(ids.len()),
    };
    // 哈希 -> 磁盘上的文件名，用于把同一张图只写一次进 zip。
    let mut packed_images: BTreeMap<String, String> = BTreeMap::new();

    for id in ids {
        let example = match store.load(id)? {
            Some(example) => example,
            None => continue,
        };
        let image_entry = match example.image_file_name() {
            Some(file_name) => {
                let hash = example.image_hash.clone().unwrap_or_default();
                if packed_images.contains_key(&file_name) {
                    Some(file_name)
                } else if store.has_image(&hash, example.image_ext.as_deref().unwrap_or("png")) {
                    packed_images.insert(file_name.clone(), file_name.clone());
                    Some(file_name)
                } else {
                    // 图片文件缺失：仍然导出元数据，导入端会显示为无图。
                    None
                }
            }
            None => None,
        };

        data.examples.push(ArchiveExample {
            id,
            position: example.position,
            title: example.title.clone(),
            image: image_entry,
            sections: (0..SECTION_COUNT)
                .map(|index| example.sections.get(index).to_string())
                .collect(),
            locked: example.locked,
            created_at: example.created_at,
            updated_at: example.updated_at,
        });
    }

    let count = data.examples.len();
    let part_path = part_path_for(target);
    {
        let file = fs::File::create(&part_path)
            .with_context(|| format!("创建导出文件失败：{}", part_path.display()))?;
        let mut zip = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated);

        zip.start_file(DATA_ENTRY, options)?;
        zip.write_all(serde_json::to_string_pretty(&data)?.as_bytes())?;

        zip.start_file(MANIFEST_ENTRY, options)?;
        zip.write_all(manifest_text(&data).as_bytes())?;

        // 图片以 stored 方式写入：它们本身已是压缩格式，再 deflate 只浪费 CPU。
        let store_options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored);
        for file_name in packed_images.keys() {
            let hash = file_name.rsplit_once('.').map(|(h, _)| h).unwrap_or(file_name);
            let ext = file_name.rsplit_once('.').map(|(_, e)| e).unwrap_or("png");
            if let Some(bytes) = store.read_image(hash, ext)? {
                zip.start_file(format!("{IMAGES_PREFIX}{file_name}"), store_options)?;
                zip.write_all(&bytes)?;
            }
        }

        let mut file = zip.finish()?;
        file.flush()?;
    }
    fs::rename(&part_path, target).with_context(|| {
        format!(
            "替换导出文件失败：{} -> {}",
            part_path.display(),
            target.display()
        )
    })?;
    Ok(count)
}

/// 从 zip 归档导入。返回报告。
pub fn import(store: &mut Store, source: &Path, mode: ImportMode) -> Result<ImportReport> {
    let file = fs::File::open(source)
        .with_context(|| format!("打开导入文件失败：{}", source.display()))?;
    let mut zip = ZipArchive::new(file)
        .with_context(|| format!("不是有效的 zip 归档：{}", source.display()))?;

    // 1) 先把所有条目的名字收齐，图片哈希必须由内容算出（而不是信任文件名）。
    let mut image_entries: Vec<(String, usize)> = Vec::new();
    for index in 0..zip.len() {
        let name = {
            let entry = zip.by_index(index)?;
            if !entry.is_file() {
                continue;
            }
            normalize_entry_name(&entry.name()?)
        };
        if let Some(rest) = name.strip_prefix(IMAGES_PREFIX) {
            if !rest.is_empty() && !rest.ends_with('/') {
                image_entries.push((rest.to_string(), index));
            }
        }
    }

    let mut image_bytes: BTreeMap<String, (Vec<u8>, &'static str)> = BTreeMap::new();
    for (file_name, index) in &image_entries {
        let mut buffer = Vec::new();
        zip.by_index(*index)?.read_to_end(&mut buffer)?;
        match ext_for_bytes(&buffer) {
            Some(ext) => {
                image_bytes.insert(file_name.clone(), (buffer, ext));
            }
            None => {
                // 无法识别的图片条目：忽略，导入后该示例显示为无图。
                continue;
            }
        }
    }

    // 2) 读元数据。
    let mut raw = Vec::new();
    zip.by_name(DATA_ENTRY)
        .with_context(|| format!("归档缺少 {DATA_ENTRY}"))?
        .read_to_end(&mut raw)?;
    let data: ArchiveData =
        serde_json::from_slice(&raw).context("解析归档元数据失败（data.json）")?;
    if data.archive_version > ARCHIVE_VERSION {
        return Err(anyhow!(
            "归档版本 {} 高于本程序支持的 {ARCHIVE_VERSION}，请升级 Prompt Gallery",
            data.archive_version
        ));
    }

    let mut report = ImportReport::default();
    let mut used_ids: HashSet<i64> = store
        .rows()
        .iter()
        .map(|row| row.id)
        .collect();
    let mut next_position = store.next_position();

    // 3) 先落图片：内容寻址，重复内容不会写第二份。
    let mut written: HashSet<String> = HashSet::new();
    for example in &data.examples {
        let Some(entry) = example.image.as_deref() else {
            continue;
        };
        let entry = image_file_name_of(entry);
        let Some((bytes, ext)) = image_bytes.get(&entry) else {
            report.missing_images += 1;
            continue;
        };
        let (hash, ext) = store.store_image_bytes(bytes, ext)?;
        if written.insert(format!("{hash}.{ext}")) {
            report.images_written += 1;
        }
    }

    // 4) 再落记录。id 冲突时另分配，绝不覆盖已有示例。
    for archived in &data.examples {
        let mut sections = Sections::new();
        for (index, text) in archived.sections.iter().enumerate() {
            if index >= SECTION_COUNT {
                break;
            }
            if let Some(slot) = sections.get_mut(index) {
                *slot = text.clone();
            }
        }

        let mut example = Example::draft(next_position);
        example.title = archived.title.clone();
        example.sections = sections;
        example.created_at = archived.created_at;
        example.updated_at = archived.updated_at;
        example.locked = archived.locked;

        if let Some(entry) = archived.image.as_deref() {
            let entry = image_file_name_of(entry);
            if let Some((bytes, detected)) = image_bytes.get(&entry) {
                let (hash, ext) = store.store_image_bytes(bytes, detected)?;
                example.image_hash = Some(hash);
                example.image_ext = Some(ext);
            }
        }

        if example.is_blank() {
            report.skipped += 1;
            continue;
        }

        let keep_id = mode == ImportMode::KeepIds
            && archived.id > 0
            && used_ids.insert(archived.id);
        if keep_id {
            store.insert_with_id(&mut example, archived.id)?;
        } else {
            if mode == ImportMode::KeepIds && archived.id > 0 {
                report.remapped_ids += 1;
            }
            store.insert(&mut example)?;
        }
        next_position += 1;
        report.added += 1;
    }

    Ok(report)
}

/// 归档里的说明文本。
fn manifest_text(data: &ArchiveData) -> String {
    let mut out = String::new();
    out.push_str("Prompt Gallery 图库导出包\n");
    out.push_str("========================\n\n");
    out.push_str(&format!("归档版本: {}\n", data.archive_version));
    out.push_str(&format!("导出时间(Unix 秒): {}\n", data.exported_at));
    out.push_str(&format!("示例数量: {}\n", data.examples.len()));
    out.push_str("\n格式说明\n--------\n");
    out.push_str("* data.json         示例元数据与五个分区的正文（不含分区标题）。\n");
    out.push_str("* images/<hash>.<ext> 图片文件，文件名是内容的 blake3 哈希，天然去重。\n");
    out.push_str("* 恢复方式：Prompt Gallery → 导入图库 → 选择本 zip。\n");
    out.push_str("\n正向提示词的五个分区按固定顺序排列：\n");
    for (index, title) in SECTION_TITLES.iter().enumerate() {
        out.push_str(&format!("  {}. {}（UI 标题，不属于提示词正文）\n", index + 1, title));
    }
    out
}

/// 半成品导出文件路径。
fn part_path_for(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "export.zip".to_string());
    name.push_str(".part");
    match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

/// 去掉 zip 条目名的前导 `./` 与 Windows 风格反斜杠。
fn normalize_entry_name(name: &str) -> String {
    let name = name.replace('\\', "/");
    let name = name.trim_start_matches("./");
    name.to_string()
}

/// 从 `data.json` 的 `image` 字段取出纯粹的文件名。
///
/// 容忍两种写法：只写文件名（当前格式），或写 `images/<文件名>`（早期格式），
/// 这样旧导出包也能导入。
fn image_file_name_of(field: &str) -> String {
    let normalized = normalize_entry_name(field);
    normalized
        .rsplit_once('/')
        .map(|(_, name)| name.to_string())
        .unwrap_or(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Example;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 测试用临时目录，`Drop` 时自动删除——断言失败 panic 时也会清理干净。
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
                "prompt-gallery-archive-{tag}-{}-{seq}-{nanos}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 1x1 PNG。
    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, b'I', b'H', b'D',
        b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89,
    ];

    #[test]
    fn export_then_import_roundtrip() {
        let dir = TempDir::new("roundtrip");
        let mut store = Store::open_in(dir.join("a")).unwrap();
        let (hash, ext) = store.store_image_bytes(PNG, "png").unwrap();
        let mut example = Example::draft(0);
        example.title = "示例一".into();
        example.image_hash = Some(hash.clone());
        example.image_ext = Some(ext);
        example.sections.get_mut(1).unwrap().push_str("1girl, solo");
        let id = store.insert(&mut example).unwrap();

        let zip_path = dir.join(default_file_name(1_704_164_645).as_str());
        assert_eq!(zip_path.file_name().unwrap(), "prompt-gallery-20240102-030405.zip");
        assert_eq!(export(&store, &zip_path).unwrap(), 1);

        // 导入到另一个全新的库。
        let mut fresh = Store::open_in(dir.join("b")).unwrap();
        let report = import(&mut fresh, &zip_path, ImportMode::Append).unwrap();
        assert_eq!(report.added, 1);
        assert_eq!(report.images_written, 1);
        assert_eq!(report.missing_images, 0);

        let rows = fresh.rows().to_vec();
        assert_eq!(rows.len(), 1);
        let loaded = fresh.load(rows[0].id).unwrap().unwrap();
        assert_eq!(loaded.title, "示例一");
        assert_eq!(loaded.sections.get(1), "1girl, solo");
        assert_eq!(loaded.sections.get(0), "");
        assert!(loaded.image_hash.is_some());
        // 图片以内容哈希命名，跨库导入后哈希一致。
        assert_eq!(loaded.image_hash.as_deref(), Some(hash.as_str()));
        assert!(loaded.id.is_some());
        let _ = id;

        // JSON 里不应出现分区标题。
        let file = fs::File::open(&zip_path).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();
        let mut raw = String::new();
        zip.by_name(DATA_ENTRY).unwrap().read_to_string(&mut raw).unwrap();
        for title in SECTION_TITLES {
            assert!(!raw.contains(title), "data.json 不应包含分区标题 {title}");
        }
    }

    #[test]
    fn image_field_accepts_both_formats() {
        assert_eq!(image_file_name_of("abc.png"), "abc.png");
        assert_eq!(image_file_name_of("images/abc.png"), "abc.png");
        assert_eq!(image_file_name_of("./images/abc.png"), "abc.png");
        assert_eq!(image_file_name_of("images\\abc.png"), "abc.png");
    }

    #[test]
    fn import_keep_ids_remaps_conflicts_instead_of_overwriting() {
        let dir = TempDir::new("conflict");
        let mut store = Store::open_in(dir.join("a")).unwrap();
        let mut example = Example::draft(0);
        example.title = "原件".into();
        let original_id = store.insert(&mut example).unwrap();

        let zip_path = dir.join("pack.zip");
        export(&store, &zip_path).unwrap();

        // 同一个库里再导一次：id 冲突，必须新增而不是覆盖。
        let report = import(&mut store, &zip_path, ImportMode::KeepIds).unwrap();
        assert_eq!(report.added, 1);
        assert_eq!(report.remapped_ids, 1);
        assert_eq!(store.row_count(), 2);
        let original = store.load(original_id).unwrap().unwrap();
        assert_eq!(original.title, "原件");
        let titles: Vec<String> = store.rows().iter().map(|r| r.title.clone()).collect();
        assert_eq!(titles, vec!["原件".to_string(), "原件".to_string()]);
    }

    #[test]
    fn export_writes_locked_and_old_json_defaults_to_unlocked() {
        let dir = TempDir::new("locked-archive");
        let mut store = Store::open_in(dir.join("a")).unwrap();
        let mut example = Example::draft(0);
        example.title = "锁定的".into();
        let id = store.insert(&mut example).unwrap();
        store.set_locked(id, true).unwrap();

        let zip_path = dir.join("pack.zip");
        assert_eq!(export(&store, &zip_path).unwrap(), 1);
        let file = fs::File::open(&zip_path).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();
        let mut raw = String::new();
        zip.by_name(DATA_ENTRY)
            .unwrap()
            .read_to_string(&mut raw)
            .unwrap();
        assert!(raw.contains("\"locked\": true"), "导出 JSON 应含 locked 字段：{raw}");
        let data: ArchiveData = serde_json::from_str(&raw).unwrap();
        assert_eq!(data.archive_version, 1, "archive_version 应保持 1");
        assert!(data.examples[0].locked);

        // 旧归档（无 locked 字段）应默认为未锁，且不报错。
        let old = r#"{"id":1,"position":0,"title":"旧","image":null,"sections":["","","","",""],"created_at":0,"updated_at":0}"#;
        let parsed: ArchiveExample = serde_json::from_str(old).unwrap();
        assert!(!parsed.locked, "旧 JSON 缺 locked 时应按未锁处理");
    }

    #[test]
    fn locked_survives_export_import_roundtrip() {
        let dir = TempDir::new("locked-roundtrip");
        let mut store = Store::open_in(dir.join("a")).unwrap();
        let mut example = Example::draft(0);
        example.title = "锁定往返".into();
        let id = store.insert(&mut example).unwrap();
        store.set_locked(id, true).unwrap();

        let zip_path = dir.join("pack.zip");
        export(&store, &zip_path).unwrap();

        let mut fresh = Store::open_in(dir.join("b")).unwrap();
        let report = import(&mut fresh, &zip_path, ImportMode::Append).unwrap();
        assert_eq!(report.added, 1);
        let loaded = fresh.load(fresh.rows()[0].id).unwrap().unwrap();
        assert!(loaded.locked, "锁定状态应随归档导入");
    }
}
