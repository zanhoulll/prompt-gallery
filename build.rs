//! 构建脚本：把程序化图标与版本信息编成 exe 资源（ADR-0010）。
//!
//! Win11 任务栏与资源管理器取可执行文件内嵌的图标资源，窗口级 WM_SETICON
//! 只影响窗口自身；所以 .ico 必须在链接期进 .rsrc 段。
//! 找不到 rc.exe 时只警告、不失败：exe 仍能构建，只是图标退回系统默认。

#[path = "src/icon_render.rs"]
mod icon_render;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 进 exe 的尺寸：16/24/32 给任务栏与小图标，48/64 给资源管理器，128/256 给大图标视图。
const ICON_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/icon_render.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out_dir.join("prompt-gallery.ico");
    fs::write(&ico, icon_render::ico(&ICON_SIZES)).expect("写 icon.ico");

    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let rc = out_dir.join("prompt-gallery.rc");
    fs::write(&rc, rc_script(&ico, &version)).expect("写 .rc");

    let Some(rc_exe) = find_rc() else {
        println!(
            "cargo:warning=找不到 rc.exe（Windows SDK），exe 不含图标资源；装 SDK 或设 RC 环境变量后重新构建"
        );
        return;
    };

    let res = out_dir.join("prompt-gallery.res");
    match Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(&rc)
        .output()
    {
        Ok(output) if output.status.success() => {
            println!("cargo:rustc-link-arg-bins={}", res.display());
        }
        Ok(output) => println!(
            "cargo:warning=rc.exe 失败（退出码 {:?}）：{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        Err(err) => println!("cargo:warning=无法运行 rc.exe：{err}"),
    }
}

/// 生成 .rc：一条图标资源 + VERSIONINFO（版本号来自 Cargo.toml）。
fn rc_script(ico: &Path, version: &str) -> String {
    let ico = ico.to_string_lossy().replace('\\', "\\\\");
    let mut parts: Vec<u32> = version
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect();
    parts.resize(4, 0);
    let numeric = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);

    format!(
        r#"1 ICON "{ico}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "Prompt Gallery"
            VALUE "FileDescription", "Prompt Gallery"
            VALUE "FileVersion", "{version}"
            VALUE "OriginalFilename", "prompt-gallery.exe"
            VALUE "ProductName", "Prompt Gallery"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    )
}

/// 找 rc.exe：先认 RC 环境变量，再扫 Windows Kits 10 的 x64 目录取最高版本。
fn find_rc() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("RC") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    let mut candidates = Vec::new();
    for base in [
        std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| "C:\\Program Files (x86)".to_string()),
        std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".to_string()),
    ] {
        let bin = Path::new(&base).join("Windows Kits").join("10").join("bin");
        let Ok(entries) = fs::read_dir(&bin) else {
            continue;
        };
        for entry in entries.flatten() {
            let rc = entry.path().join("x64").join("rc.exe");
            if rc.is_file() {
                candidates.push(rc);
            }
        }
    }
    candidates.sort();
    candidates.pop()
}
