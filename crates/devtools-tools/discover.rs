//! 构建脚本与集成测试共用的目录发现，不解析业务代码。

use std::{fs, io, path::Path};

pub fn generate(root: &Path) -> io::Result<String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err(io::Error::other("工具目录不允许符号链接"));
        }
        if !kind.is_dir() {
            continue;
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| io::Error::other("工具目录名必须是 UTF-8"))?;
        if !name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        {
            return Err(io::Error::other(format!("工具目录名无效：{name}")));
        }
        if !entry.path().join("mod.rs").is_file()
            || fs::symlink_metadata(entry.path().join("mod.rs"))?
                .file_type()
                .is_symlink()
            || entry.path().join("Cargo.toml").exists()
        {
            return Err(io::Error::other(format!(
                "工具 {name} 必须有 mod.rs 且不能有 Cargo.toml"
            )));
        }
        names.push(name);
    }
    names.sort();
    let mut code = String::new();
    for name in &names {
        let path = fs::canonicalize(root.join(name).join("mod.rs"))?;
        let path = path
            .to_str()
            .ok_or_else(|| io::Error::other("工具路径必须是 UTF-8"))?;
        code.push_str(&format!("#[path = {path:?}] mod tool_{name};\n"));
    }
    code.push_str("fn discovered() -> Vec<devtools_module_api::ModuleDescriptor> { vec![\n");
    for name in names {
        code.push_str(&format!("tool_{name}::descriptor(),\n"));
    }
    code.push_str("] }\n");
    Ok(code)
}
