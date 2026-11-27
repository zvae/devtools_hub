//! 直接编译生成结果，验证只增删目录即可更新注册，不依赖中央注册项。

#[path = "../discover.rs"]
mod discover;

fn compile_and_read(root: &std::path::Path) -> String {
    let generated = discover::generate(&root.join("tools")).unwrap();
    let source = format!(
        r#"
extern crate self as devtools_module_api;
pub struct ModuleDescriptor {{ pub id: &'static str }}
{generated}
fn main() {{ for d in discovered() {{ println!("{{}}", d.id); }} }}
"#
    );
    std::fs::write(root.join("probe.rs"), source).unwrap();
    let binary = root.join(format!("probe{}", std::env::consts::EXE_SUFFIX));
    let result = std::process::Command::new("rustc")
        .arg("--edition=2021")
        .arg(root.join("probe.rs"))
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(std::process::Command::new(binary).output().unwrap().stdout).unwrap()
}

fn add(root: &std::path::Path, name: &str) {
    let dir = root.join("tools").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("mod.rs"), "//! 内部模块与真实源码路径共同编译。\nmod model;\npub(super) fn descriptor() -> devtools_module_api::ModuleDescriptor { devtools_module_api::ModuleDescriptor { id: model::ID } }").unwrap();
    std::fs::write(
        dir.join("model.rs"),
        format!("pub(super) const ID: &str = {name:?};"),
    )
    .unwrap();
}

#[test]
fn adding_and_removing_directories_changes_compiled_registry() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("含 空格的目录");
    add(&root, "alpha");
    assert_eq!(compile_and_read(&root), "alpha\n");
    add(&root, "beta");
    assert_eq!(compile_and_read(&root), "alpha\nbeta\n");
    std::fs::remove_dir_all(root.join("tools/alpha")).unwrap();
    assert_eq!(compile_and_read(&root), "beta\n");
}

#[test]
fn malformed_directories_fail_instead_of_silently_disappearing() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir(root.join("Bad-Name")).unwrap();
    assert!(discover::generate(root).is_err());
    std::fs::remove_dir(root.join("Bad-Name")).unwrap();
    std::fs::create_dir(root.join("valid")).unwrap();
    assert!(discover::generate(root).is_err());
}
