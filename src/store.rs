//! 数据库（Database）：一个 SQLite 文件存示例元数据与各分区文本，
//! 一个托管 `images/` 目录存图片文件，以内容哈希命名、天然去重（ADR-0003）。
//!
//! 两者整体位于程序目录旁的 `data/` 下；默认落在 `%APPDATA%\PromptGallery\`，
//! 可用环境变量 `PROMPT_GALLERY_DATA_DIR` 覆盖（便于便携部署与测试）。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::{now_unix, Example, Sections, SECTION_COUNT};

/// 数据库文件名。
pub const DB_FILE_NAME: &str = "gallery.sqlite3";
/// 托管图片目录名。
pub const IMAGES_DIR_NAME: &str = "images";
/// 当前 schema 版本，写入 SQLite 的 `PRAGMA user_version`。
const SCHEMA_VERSION: i64 = 2;

/// 索引用的轻量条目：侧边栏只需要标题与图片，不必把五个分区文本全读出来。
#[derive(Clone, Debug)]
pub struct Row {
    pub id: i64,
    pub title: String,
    pub image_hash: Option<String>,
    pub image_ext: Option<String>,
    /// 只读锁（ADR-0007）。
    pub locked: bool,
}

impl Row {
    pub fn display_title(&self) -> &str {
        let trimmed = self.title.trim();
        if trimmed.is_empty() {
            "未命名示例"
        } else {
            trimmed
        }
    }

    pub fn image_file_name(&self) -> Option<String> {
        match (&self.image_hash, &self.image_ext) {
            (Some(hash), Some(ext)) => Some(format!("{hash}.{ext}")),
            _ => None,
        }
    }
}

pub struct Store {
    conn: Connection,
    data_dir: PathBuf,
    /// 侧边栏顺序；与数据库的 `position` 保持一致。
    rows: Vec<Row>,
}

impl Store {
    /// 打开（必要时初始化）数据库。
    pub fn open() -> Result<Self> {
        let data_dir = resolve_data_dir()?;
        Self::open_in(data_dir)
    }

    /// 在指定目录下打开数据库；目录不存在则创建。
    pub fn open_in(data_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&data_dir)
            .with_context(|| format!("创建数据目录失败：{}", data_dir.display()))?;
        fs::create_dir_all(data_dir.join(IMAGES_DIR_NAME))
            .with_context(|| format!("创建图片目录失败：{}", data_dir.display()))?;

        let db_path = data_dir.join(DB_FILE_NAME);
        let conn = Connection::open(&db_path)
            .with_context(|| format!("打开数据库失败：{}", db_path.display()))?;

        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;

        let mut store = Self {
            conn,
            data_dir,
            rows: Vec::new(),
        };
        store.reload_rows()?;
        Ok(store)
    }

    /// 数据库文件所在目录。
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// 托管 `images/` 目录。
    pub fn images_dir(&self) -> PathBuf {
        self.data_dir.join(IMAGES_DIR_NAME)
    }

    /// 侧边栏顺序的条目列表。
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// 下一个示例的排序位。
    pub fn next_position(&self) -> i64 {
        self.conn
            .query_row("SELECT COALESCE(MAX(position), -1) + 1 FROM examples", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap_or(0)
    }

    /// 按 id 读取一条完整示例。
    pub fn load(&self, id: i64) -> Result<Option<Example>> {
        let mut example = self
            .conn
            .query_row(
                "SELECT id, position, title, image_hash, image_ext, locked, created_at, updated_at
                 FROM examples WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Example {
                        id: Some(row.get(0)?),
                        position: row.get(1)?,
                        title: row.get(2)?,
                        image_hash: row.get(3)?,
                        image_ext: row.get(4)?,
                        sections: Sections::new(),
                        locked: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                },
            )
            .optional()?;

        if let Some(example) = example.as_mut() {
            load_sections(&self.conn, example)?;
        }
        Ok(example)
    }

    /// 追加新示例到列表末尾，返回新 id。
    ///
    /// 导入归档与开发用 seed 走这里，保持「追加」语义（§8 有意的不对称）。
    /// 图片必须先由 [`Store::store_image_bytes`] 落盘，这里只记哈希。
    pub fn insert(&mut self, example: &mut Example) -> Result<i64> {
        self.insert_inner(example, None, false)
    }

    /// 以指定 id 插入（导入归档时沿用原 id），同样追加到末尾。
    pub fn insert_with_id(&mut self, example: &mut Example, id: i64) -> Result<i64> {
        self.insert_inner(example, Some(id), false)
    }

    /// 草稿落库：新示例占 position 0，其余示例依次后移（§8「保存后留在顶部」）。
    pub fn insert_at_top(&mut self, example: &mut Example) -> Result<i64> {
        self.insert_inner(example, None, true)
    }

    fn insert_inner(
        &mut self,
        example: &mut Example,
        forced_id: Option<i64>,
        at_top: bool,
    ) -> Result<i64> {
        let now = now_unix();
        if !at_top && example.position == 0 && self.row_count() > 0 {
            example.position = self.next_position();
        }
        let sections = example.sections.clone();
        let title = example.title.trim().to_string();
        let image_hash = example.image_hash.clone();
        let image_ext = example.image_ext.clone();
        let locked = example.locked;
        let mut position = example.position;

        let tx = self.conn.transaction()?;
        if at_top {
            // 现有示例整体后移一位，给草稿腾出 position 0。
            tx.execute("UPDATE examples SET position = position + 1", [])?;
            position = 0;
        }
        let id = match forced_id {
            Some(id) => {
                tx.execute(
                    "INSERT INTO examples (id, position, title, image_hash, image_ext, locked, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    params![id, position, title, image_hash, image_ext, locked, now],
                )?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO examples (position, title, image_hash, image_ext, locked, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                    params![position, title, image_hash, image_ext, locked, now],
                )?;
                tx.last_insert_rowid()
            }
        };
        write_sections_tx(&tx, id, &sections)?;
        tx.commit()?;

        example.id = Some(id);
        example.position = position;
        example.title = title;
        example.created_at = now;
        example.updated_at = now;
        self.reload_rows()?;
        Ok(id)
    }

    /// 更新已落库示例的标题、分区文本与图片。
    pub fn update(&mut self, example: &mut Example) -> Result<()> {
        let Some(id) = example.id else {
            anyhow::bail!("不能更新未落库的草稿");
        };
        if self.is_locked(id)? {
            anyhow::bail!("示例已锁定，无法修改");
        }
        let now = now_unix();
        let title = example.title.trim().to_string();
        let image_hash = example.image_hash.clone();
        let image_ext = example.image_ext.clone();
        let sections = example.sections.clone();

        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE examples SET title = ?2, image_hash = ?3, image_ext = ?4, updated_at = ?5
             WHERE id = ?1",
            params![id, title, image_hash, image_ext, now],
        )?;
        tx.execute("DELETE FROM sections WHERE example_id = ?1", params![id])?;
        write_sections_tx(&tx, id, &sections)?;
        tx.commit()?;

        example.title = title;
        example.updated_at = now;
        self.reload_rows()?;
        Ok(())
    }

    /// 删除示例。图片文件是内容寻址、可能被多条示例共享，因此保留不删。
    pub fn delete(&mut self, id: i64) -> Result<()> {
        if self.is_locked(id)? {
            anyhow::bail!("示例已锁定，无法删除");
        }
        self.conn
            .execute("DELETE FROM examples WHERE id = ?1", params![id])?;
        self.reload_rows()?;
        Ok(())
    }

    /// 该示例当前是否已锁定；不存在的 id 视为未锁。
    fn is_locked(&self, id: i64) -> Result<bool> {
        let locked = self
            .conn
            .query_row(
                "SELECT locked FROM examples WHERE id = ?1",
                params![id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?;
        Ok(locked.unwrap_or(false))
    }

    /// 开 / 关示例锁（ADR-0007）。
    ///
    /// 只写 locked 与 updated_at，故意不走 Store::update——否则会被自身的
    /// 锁定校验挡住。拖动排序（Store::reorder）不受锁影响。
    pub fn set_locked(&mut self, id: i64, locked: bool) -> Result<()> {
        let now = now_unix();
        self.conn.execute(
            "UPDATE examples SET locked = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, locked, now],
        )?;
        self.reload_rows()?;
        Ok(())
    }

    /// 按给定顺序重排并落库排序位。
    pub fn reorder(&mut self, ordered_ids: &[i64]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (index, id) in ordered_ids.iter().enumerate() {
            tx.execute(
                "UPDATE examples SET position = ?2 WHERE id = ?1",
                params![id, index as i64],
            )?;
        }
        tx.commit()?;
        self.reload_rows()?;
        Ok(())
    }

    /// 图片是否已在托管目录中。
    pub fn has_image(&self, hash: &str, ext: &str) -> bool {
        self.images_dir().join(format!("{hash}.{ext}")).is_file()
    }

    /// 读取图片文件字节；文件缺失返回 `Ok(None)`。
    pub fn read_image(&self, hash: &str, ext: &str) -> Result<Option<Vec<u8>>> {
        let path = self.images_dir().join(format!("{hash}.{ext}"));
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("读取图片失败：{}", path.display())),
        }
    }

    /// 把图片字节写入托管目录，返回（内容哈希, 扩展名）。
    ///
    /// 文件名即内容哈希，因此重复导入同一张图片不会产生第二个文件（ADR-0003）。
    pub fn store_image_bytes(&self, bytes: &[u8], ext: &str) -> Result<(String, String)> {
        let hash = blake3::hash(bytes).to_hex().to_string();
        let ext = normalize_ext(ext);
        let path = self.images_dir().join(format!("{hash}.{ext}"));
        if !path.is_file() {
            fs::write(&path, bytes)
                .with_context(|| format!("写入图片失败：{}", path.display()))?;
        }
        Ok((hash, ext))
    }

    /// 重新读取侧边栏列表。
    fn reload_rows(&mut self) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, image_hash, image_ext, locked FROM examples ORDER BY position ASC, id ASC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Row {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    image_hash: row.get(2)?,
                    image_ext: row.get(3)?,
                    locked: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        self.rows = rows;
        Ok(())
    }
}

/// 建表并迁移到 [`SCHEMA_VERSION`]。
fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    if version == 0 {
        // 新库：直接按 v2 的形状建表。
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS examples (
                 id          INTEGER PRIMARY KEY,
                 position    INTEGER NOT NULL DEFAULT 0,
                 title       TEXT    NOT NULL DEFAULT '',
                 image_hash  TEXT,
                 image_ext   TEXT,
                 locked      INTEGER NOT NULL DEFAULT 0,
                 created_at  INTEGER NOT NULL DEFAULT 0,
                 updated_at  INTEGER NOT NULL DEFAULT 0
             );
             CREATE INDEX IF NOT EXISTS idx_examples_position ON examples(position);
             -- 分区文本按索引 0..4 存行；分区标题是 UI 常量，不入库。
             CREATE TABLE IF NOT EXISTS sections (
                 example_id INTEGER NOT NULL REFERENCES examples(id) ON DELETE CASCADE,
                 section_index INTEGER NOT NULL,
                 text       TEXT    NOT NULL DEFAULT '',
                 PRIMARY KEY (example_id, section_index)
             );
             PRAGMA user_version = 2;
             COMMIT;",
        )
        .context("初始化数据库 schema 失败")?;
    } else {
        // 旧库（v1）：只补 locked 列；已有示例全部保持未锁，数据不动，只跑一次。
        conn.execute_batch(
            "BEGIN;
             ALTER TABLE examples ADD COLUMN locked INTEGER NOT NULL DEFAULT 0;
             PRAGMA user_version = 2;
             COMMIT;",
        )
        .context("升级数据库 schema 到 v2 失败")?;
    }
    Ok(())
}

fn write_sections_tx(
    tx: &rusqlite::Transaction<'_>,
    example_id: i64,
    sections: &Sections,
) -> Result<()> {
    let mut stmt = tx.prepare(
        "INSERT INTO sections (example_id, section_index, text) VALUES (?1, ?2, ?3)",
    )?;
    for (index, text) in sections.iter() {
        // 空分区不落行，读回时按空串补齐。
        if text.trim().is_empty() {
            continue;
        }
        stmt.execute(params![example_id, index as i64, text])?;
    }
    Ok(())
}

fn load_sections(conn: &Connection, example: &mut Example) -> Result<()> {
    let mut stmt = conn.prepare("SELECT section_index, text FROM sections WHERE example_id = ?1")?;
    let rows = stmt.query_map(params![example.id], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (index, text) = row?;
        if index >= 0 && (index as usize) < SECTION_COUNT {
            if let Some(slot) = example.sections.get_mut(index as usize) {
                *slot = text;
            }
        }
    }
    Ok(())
}

/// 统一扩展名写法：小写、无点。
pub fn normalize_ext(ext: &str) -> String {
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    match ext.as_str() {
        "jpeg" => "jpg".to_string(),
        "tif" => "tiff".to_string(),
        "" => "bin".to_string(),
        other => other.to_string(),
    }
}

/// 内容哈希对应的文件扩展名，用于按图片内容而非文件名判断格式。
pub fn ext_for_bytes(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.starts_with(b"BM") {
        Some("bmp")
    } else if bytes.starts_with(&[0x49, 0x49, 0x2A, 0x00]) || bytes.starts_with(&[0x4D, 0x4D, 0x00, 0x2A])
    {
        Some("tiff")
    } else {
        None
    }
}

/// 解析数据目录：优先 `PROMPT_GALLERY_DATA_DIR`，否则 `%APPDATA%\PromptGallery\`。
///
/// 目录名为应用显示名 `Prompt Gallery` 的紧凑形式，与窗口标题保持一致。
pub fn resolve_data_dir() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("PROMPT_GALLERY_DATA_DIR") {
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData/Roaming")))
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(base.join("PromptGallery"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 测试用临时目录，`Drop` 时自动删除。
    ///
    /// 关键点是**不依赖测试正常结束**：断言失败会 panic，析构函数照样运行，
    /// 因此 `%TEMP%` 里不会残留目录。名字里带进程号与计数器，同一进程内复用互不打架。
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
                "prompt-gallery-test-{tag}-{}-{seq}-{nanos}",
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

    #[test]
    fn roundtrip_example_with_hashed_image() {
        let dir = TempDir::new("roundtrip");
        let mut store = Store::open_in(dir.path().to_path_buf()).unwrap();

        let bytes = [0x89, b'P', b'N', b'G', 1, 2, 3, 4];
        let (hash, ext) = store.store_image_bytes(&bytes, "png").unwrap();
        assert_eq!(ext, "png");
        assert!(store.has_image(&hash, &ext));

        let mut example = Example::draft(0);
        example.title = "  测试示例  ".to_string();
        example.image_hash = Some(hash.clone());
        example.image_ext = Some(ext.clone());
        example.sections.get_mut(0).unwrap().push_str("best quality");
        example.sections.get_mut(4).unwrap().push_str("rim light");

        let id = store.insert(&mut example).unwrap();
        assert!(id > 0);

        let loaded = store.load(id).unwrap().unwrap();
        assert_eq!(loaded.title, "测试示例");
        assert_eq!(loaded.sections.get(0), "best quality");
        assert_eq!(loaded.sections.get(4), "rim light");
        assert_eq!(loaded.sections.get(1), "");
        assert_eq!(loaded.image_file_name(), Some(format!("{hash}.png")));

        // 同一份字节再存一次不应产生新文件。
        let (hash2, _) = store.store_image_bytes(&bytes, "png").unwrap();
        assert_eq!(hash, hash2);
        let files = fs::read_dir(store.images_dir()).unwrap().count();
        assert_eq!(files, 1);

        store.delete(id).unwrap();
        assert_eq!(store.row_count(), 0);
        // 内容寻址的图片在删除示例后保留。
        assert!(store.has_image(&hash, "png"));
    }

    #[test]
    fn reorder_persists() {
        let dir = TempDir::new("reorder");
        let mut store = Store::open_in(dir.path().to_path_buf()).unwrap();
        let mut a = Example::draft(0);
        a.title = "A".into();
        let mut b = Example::draft(0);
        b.title = "B".into();
        let a_id = store.insert(&mut a).unwrap();
        let b_id = store.insert(&mut b).unwrap();
        assert_eq!(store.rows()[0].id, a_id);

        store.reorder(&[b_id, a_id]).unwrap();
        assert_eq!(store.rows()[0].id, b_id);
        assert_eq!(store.rows()[1].id, a_id);
    }

    #[test]
    fn detects_image_format_from_magic_bytes() {
        assert_eq!(ext_for_bytes(&[0x89, b'P', b'N', b'G', 0x0D]), Some("png"));
        assert_eq!(ext_for_bytes(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(ext_for_bytes(b"GIF89a"), Some("gif"));
        assert_eq!(ext_for_bytes(b"nope"), None);
    }

    /// 临时目录守卫的回归测试：断言失败（panic）时也必须清理干净。
    ///
    /// 把一个本身会 panic 的守卫放进 `catch_unwind`，等价于「测试中途失败」，
    /// 用来证明 `Drop` 确实在 unwind 期间运行——这正是之前 `%TEMP%` 里堆了
    /// 二十多个残留目录的根因。
    #[test]
    fn temp_dir_guard_cleans_up_on_panic() {
        let mut leaked: Option<PathBuf> = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let dir = TempDir::new("panic-guard");
            leaked = Some(dir.path().to_path_buf());
            assert!(dir.path().is_dir(), "守卫应先创建目录");
            panic!("模拟测试中途失败");
        }));
        let path = leaked.expect("panic 之前应已记录路径");
        assert!(result.is_err(), "闭包应当 panic");
        assert!(
            !path.exists(),
            "断言失败后临时目录仍残留：{}",
            path.display()
        );
    }

    #[test]
    fn new_database_has_locked_column_and_version_2() {
        let dir = TempDir::new("schema-new");
        let _store = Store::open_in(dir.path().to_path_buf()).unwrap();
        let conn = Connection::open(dir.path().join(DB_FILE_NAME)).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);
        // SELECT locked 能跑通即证明列存在。
        let count: i64 = conn
            .query_row("SELECT COUNT(locked) FROM examples", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn old_database_migrates_to_v2_keeping_rows_unlocked() {
        let dir = TempDir::new("schema-old");
        // TempDir 只算路径、不建目录；手工造库前先建好。
        fs::create_dir_all(dir.path()).unwrap();
        let db_path = dir.path().join(DB_FILE_NAME);
        {
            // 手工造一个 v1 旧库：没有 locked 列。
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE examples (
                     id INTEGER PRIMARY KEY, position INTEGER NOT NULL DEFAULT 0,
                     title TEXT NOT NULL DEFAULT '', image_hash TEXT, image_ext TEXT,
                     created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0);
                 CREATE TABLE sections (
                     example_id INTEGER NOT NULL REFERENCES examples(id) ON DELETE CASCADE,
                     section_index INTEGER NOT NULL, text TEXT NOT NULL DEFAULT '',
                     PRIMARY KEY (example_id, section_index));
                 INSERT INTO examples (id, position, title) VALUES (7, 0, '旧示例');
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let store = Store::open_in(dir.path().to_path_buf()).unwrap();
        let version: i64 = store
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2, "旧库应升级到 v2");
        let loaded = store.load(7).unwrap().unwrap();
        assert_eq!(loaded.title, "旧示例", "迁移不应丢数据");
        assert!(!loaded.locked, "老示例迁移后应保持未锁");
        assert_eq!(store.row_count(), 1);
    }

    #[test]
    fn locked_rows_reject_update_and_delete_but_reorder_and_unlock_work() {
        let dir = TempDir::new("lock");
        let mut store = Store::open_in(dir.path().to_path_buf()).unwrap();
        let mut a = Example::draft(0);
        a.title = "A".into();
        let id = store.insert(&mut a).unwrap();

        store.set_locked(id, true).unwrap();
        assert!(store.rows()[0].locked);

        let mut loaded = store.load(id).unwrap().unwrap();
        loaded.title = "改过".into();
        assert!(store.update(&mut loaded).is_err(), "锁定行不应能更新");
        assert!(store.delete(id).is_err(), "锁定行不应能删除");
        assert_eq!(store.load(id).unwrap().unwrap().title, "A", "锁定后内容不应被改");
        // 拖动排序不受锁影响（ADR-0007）。
        assert!(store.reorder(&[id]).is_ok());

        store.set_locked(id, false).unwrap();
        assert!(!store.rows()[0].locked);
        let mut loaded = store.load(id).unwrap().unwrap();
        loaded.title = "改过".into();
        store.update(&mut loaded).unwrap();
        assert_eq!(store.load(id).unwrap().unwrap().title, "改过");
        store.delete(id).unwrap();
        assert_eq!(store.row_count(), 0);
    }
}


