//! 目录模块的格式化入口，避免 rustfmt 未遍历 OUT_DIR 引用的源码。

use std::{fs, path::Path, process::Command};

fn visit(path: &Path, files: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            continue;
        }
        if entry.file_type()?.is_dir() {
            visit(&entry.path(), files)?;
        } else if entry.path().extension().is_some_and(|e| e == "rs") {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|a| a != "--check") {
        return Err("仅支持 --check；无参数时格式化".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    visit(&root.join("src"), &mut files)?;
    visit(&root.join("tests"), &mut files)?;
    files.extend([
        root.join("build.rs"),
        root.join("discover.rs"),
        root.join("format.rs"),
    ]);
    files.sort();
    for file in files {
        let status = Command::new("rustfmt")
            .args(["--edition", "2021", "--config", "skip_children=true"])
            .args(&args)
            .arg(&file)
            .status()?;
        if !status.success() {
            return Err(format!("格式检查失败：{}", file.display()).into());
        }
    }
    Ok(())
}
