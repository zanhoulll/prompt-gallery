//! 领域模型：示例（Example）、草稿（Draft）与五个分区（Section）。
//!
//! 术语与 `CONTEXT.md` 的 Language 章节一一对应；本文件是这些术语在代码里的唯一落点。

use std::path::PathBuf;

/// 一个分区在数组里的下标；数组长度恒为 [`SECTION_COUNT`]。
pub type SectionIndex = usize;

/// 分区数量：正向提示词固定由五个分区组成（CONTEXT.md「分区」）。
pub const SECTION_COUNT: usize = 5;

/// 每个分区的固定标题。灰色、左对齐、不可修改、不可选中；
/// 复制提示词时被剔除，不进入剪贴板。
pub const SECTION_TITLES: [&str; SECTION_COUNT] = [
    "质量与基础",
    "人物特征",
    "服装与道具",
    "姿势与动作",
    "环境与光影",
];

/// 一个字对齐五个分区文本的容器。
///
/// 用固定长度的数组而不是 `Vec`/`HashMap`：分区标题不可变，任何「少一个/多一个分区」
/// 在编译期就不可能表达。
#[derive(Clone, PartialEq, Eq, Default, Debug)]
pub struct Sections([String; SECTION_COUNT]);

impl Sections {
    pub fn new() -> Self {
        Self::default()
    }

    /// 越界下标返回空串，避免 UI 层的索引错误把程序打崩。
    pub fn get(&self, index: SectionIndex) -> &str {
        self.0.get(index).map(String::as_str).unwrap_or("")
    }

    pub fn get_mut(&mut self, index: SectionIndex) -> Option<&mut String> {
        self.0.get_mut(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = (SectionIndex, &str)> {
        self.0.iter().enumerate().map(|(i, s)| (i, s.as_str()))
    }

    /// 五个分区是否全为空。
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|s| s.trim().is_empty())
    }

    /// 拼出可复制的正向提示词：只含正文，不含分区标题，不含分隔符。
    ///
    /// 空分区被跳过，分区之间以单个空行分隔，结果首尾无多余空白。
    pub fn join_for_clipboard(&self) -> String {
        let mut out = String::new();
        for text in self.0.iter() {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(trimmed);
        }
        out
    }

    /// 是否与另一组分区文本内容相同（忽略首尾空白）。
    pub fn same_content_as(&self, other: &Sections) -> bool {
        (0..SECTION_COUNT).all(|i| self.get(i).trim() == other.get(i).trim())
    }
}

/// 图库中的一条示例：一个标题、一张图片、五个分区的文本。
#[derive(Clone, Debug)]
pub struct Example {
    /// SQLite 主键；未落库的草稿为 `None`。
    pub id: Option<i64>,
    /// 侧边栏排序位；新示例排在末尾。
    pub position: i64,
    pub title: String,
    /// 图片文件的内容哈希（blake3 十六进制）；无图片为 `None`。
    /// `images/` 目录里对应的文件名为 `<hash>.<ext>`（ADR-0003）。
    pub image_hash: Option<String>,
    /// 图片文件扩展名，与 `image_hash` 成对出现。
    pub image_ext: Option<String>,
    pub sections: Sections,
    pub created_at: i64,
    pub updated_at: i64,
    /// 只读锁：锁定的示例不能被修改或删除（ADR-0007）。
    pub locked: bool,
}

impl Default for Example {
    fn default() -> Self {
        let now = now_unix();
        Self {
            id: None,
            position: 0,
            title: String::new(),
            image_hash: None,
            image_ext: None,
            sections: Sections::new(),
            created_at: now,
            updated_at: now,
            locked: false,
        }
    }
}

impl Example {
    /// 通过侧边栏 `+` 新建的草稿：尚未保存过，没有 id、没有图片。
    pub fn draft(position: i64) -> Self {
        Self {
            position,
            ..Self::default()
        }
    }

    /// 是否为草稿：尚未落库。
    pub fn is_draft(&self) -> bool {
        self.id.is_none()
    }

    /// 标题：草稿允许为空，UI 用 `display_title` 兜底。
    pub fn display_title(&self) -> &str {
        let trimmed = self.title.trim();
        if trimmed.is_empty() {
            "未命名示例"
        } else {
            trimmed
        }
    }

    /// 图片文件的文件名（`<hash>.<ext>`）。
    pub fn image_file_name(&self) -> Option<String> {
        match (&self.image_hash, &self.image_ext) {
            (Some(hash), Some(ext)) => Some(format!("{hash}.{ext}")),
            _ => None,
        }
    }

    /// 图片在托管 `images/` 目录下的完整路径。
    pub fn image_path(&self, images_dir: &std::path::Path) -> Option<PathBuf> {
        self.image_file_name().map(|name| images_dir.join(name))
    }

    /// 是否内容为空——空示例不值得落库。
    pub fn is_blank(&self) -> bool {
        self.title.trim().is_empty() && self.sections.is_empty() && self.image_hash.is_none()
    }

    /// 与基线（数据库里的那一份）相比是否有未保存的改动。
    pub fn differs_from(&self, baseline: &Example) -> bool {
        self.title.trim() != baseline.title.trim()
            || !self.sections.same_content_as(&baseline.sections)
            || self.image_hash != baseline.image_hash
            || self.image_ext != baseline.image_ext
    }
}

/// 当前时间（Unix 秒）。
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 把 Unix 秒格式化成 `YYYY-MM-DD HH:MM:SS`（本地时区偏移不处理，仅用于展示）。
pub fn format_unix(secs: i64) -> String {
    let (y, m, d, hh, mm, ss) = civil_from_unix(secs);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
}

/// 导出文件名前缀用的紧凑时间戳：`YYYYMMDD-HHMMSS`。
pub fn compact_timestamp(secs: i64) -> String {
    let (y, m, d, hh, mm, ss) = civil_from_unix(secs);
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

/// 把 Unix 秒拆成公历日期时间（UTC）。避免为一个时间戳引入 chrono 依赖。
fn civil_from_unix(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = secs.max(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (hh, mm, ss) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    // Howard Hinnant 的 civil_from_days 算法。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, hh, mm, ss)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_text_has_no_section_headers() {
        let mut sections = Sections::new();
        sections.get_mut(0).unwrap().push_str("  masterpiece, best quality  ");
        sections.get_mut(3).unwrap().push_str("standing");
        let text = sections.join_for_clipboard();
        assert_eq!(text, "masterpiece, best quality\n\nstanding");
        for title in SECTION_TITLES {
            assert!(!text.contains(title));
        }
    }

    #[test]
    fn empty_sections_copy_to_empty_string() {
        assert!(Sections::new().join_for_clipboard().is_empty());
    }

    #[test]
    fn timestamp_is_compact_and_stable() {
        // 2024-01-02 03:04:05 UTC
        assert_eq!(compact_timestamp(1_704_164_645), "20240102-030405");
    }

    #[test]
    fn locked_defaults_false_and_is_not_part_of_content_diff() {
        assert!(!Example::default().locked);
        assert!(!Example::draft(0).locked);

        let mut example = Example::default();
        example.id = Some(1);
        let baseline = example.clone();
        assert!(!example.differs_from(&baseline));

        // 开关锁不算内容改动（ADR-0007）。
        example.locked = true;
        assert!(!example.differs_from(&baseline));
    }
}
