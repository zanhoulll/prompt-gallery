//! 领域模型：示例（Example）、草稿（Draft）与正/负向提示词。
//!
//! 术语与 `CONTEXT.md` 的 Language 章节一一对应；本文件是这些术语在代码里的唯一落点。

use std::path::PathBuf;

/// 提示词的来源：决定锁定能否解除（ADR-0012「来源锁」）。
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum PromptSource {
    /// 用户自己写的，或从别处粘进来的。
    #[default]
    Manual,
    /// 从图片元数据里读出来的；由此派生出永久只读。
    Image,
}

impl PromptSource {
    /// 写进数据库 / `data.json` 的稳定字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Image => "image",
        }
    }

    /// 读库时的宽松解析：认不出的一律当 `Manual`。
    ///
    /// 宁可少锁，也不把一条读不懂的旧记录判成解不开的死锁。
    pub fn from_str_loose(raw: &str) -> Self {
        if raw.trim().eq_ignore_ascii_case("image") {
            Self::Image
        } else {
            Self::Manual
        }
    }

    /// 是否为来源锁：界面不画解锁按钮，`Store::set_locked(false)` 拒绝。
    pub fn is_locked_source(self) -> bool {
        self == Self::Image
    }
}

/// 图库中的一条示例：一个标题、一张图片、一段正向提示词、一段负向提示词。
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
    /// 正向提示词：一段连续文本，不再分区（ADR-0011）。
    pub positive: String,
    /// 负向提示词：单段、可留空，与正向同权。
    pub negative: String,
    /// 提示词来源；`Image` 时 `locked` 必然为 `true`（ADR-0012）。
    pub prompt_source: PromptSource,
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
            positive: String::new(),
            negative: String::new(),
            prompt_source: PromptSource::Manual,
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

    /// 两段提示词是否都为空。
    pub fn prompts_empty(&self) -> bool {
        self.positive.trim().is_empty() && self.negative.trim().is_empty()
    }

    /// 是否内容为空——空示例不值得落库。
    pub fn is_blank(&self) -> bool {
        self.title.trim().is_empty() && self.prompts_empty() && self.image_hash.is_none()
    }

    /// 与基线（数据库里的那一份）相比是否有未保存的改动。
    pub fn differs_from(&self, baseline: &Example) -> bool {
        self.title.trim() != baseline.title.trim()
            || self.positive.trim() != baseline.positive.trim()
            || self.negative.trim() != baseline.negative.trim()
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
    fn prompt_source_roundtrips_and_is_lenient() {
        assert_eq!(PromptSource::Manual.as_str(), "manual");
        assert_eq!(PromptSource::Image.as_str(), "image");
        for source in [PromptSource::Manual, PromptSource::Image] {
            assert_eq!(PromptSource::from_str_loose(source.as_str()), source);
        }
        // 读不懂的值不许变成死锁。
        assert_eq!(PromptSource::from_str_loose(""), PromptSource::Manual);
        assert_eq!(PromptSource::from_str_loose("IMAGE "), PromptSource::Image);
        assert_eq!(PromptSource::from_str_loose("weird"), PromptSource::Manual);
        assert!(PromptSource::Image.is_locked_source());
        assert!(!PromptSource::Manual.is_locked_source());
        assert_eq!(PromptSource::default(), PromptSource::Manual);
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

    #[test]
    fn both_prompts_take_part_in_blank_and_diff() {
        let mut example = Example::default();
        assert!(example.is_blank());

        example.positive = "  masterpiece  ".into();
        assert!(!example.is_blank());

        let baseline = example.clone();
        assert!(!example.differs_from(&baseline));

        example.negative = "lowres".into();
        assert!(example.differs_from(&baseline));

        // 只改空白不算改动。
        let mut spaced = baseline.clone();
        spaced.positive = "masterpiece\n\n".into();
        assert!(!spaced.differs_from(&baseline));
    }
}
