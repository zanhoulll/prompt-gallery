//! Prompt Gallery 的核心库。
//!
//! 界面在二进制 crate（`src/main.rs`），这里只放与 GUI 无关的领域逻辑，
//! 这样单元测试与开发工具（`src/bin/seed.rs`）都能直接复用同一套实现。
//!
//! 模块划分对应 `CONTEXT.md` 的术语：
//! * [`model`] —— 示例（Example）、草稿（Draft）、分区（Section）
//! * [`store`] —— 数据库（Database）：SQLite + 托管 `images/`
//! * [`archive`] —— 导出/导入 zip 归档
//! * [`images`] —— 图片解码与纹理缓存（GUI 侧的低占用取舍，见 ADR-0001）

pub mod archive;
pub mod images;
pub mod model;
pub mod store;
