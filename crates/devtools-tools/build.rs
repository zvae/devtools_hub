//! 扫描目录生成编译期注册列表；不修改源码、依赖清单或启动嵌套 Cargo。

#[path = "discover.rs"]
mod discover;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=discover.rs");
    println!("cargo:rerun-if-changed=src/tools");
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let code = discover::generate(&root.join("src/tools")).expect("发现工具失败");
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("tools_registry.rs");
    if std::fs::read_to_string(&output).ok().as_deref() != Some(&code) {
        std::fs::write(output, code).expect("生成工具注册代码失败");
    }
}
