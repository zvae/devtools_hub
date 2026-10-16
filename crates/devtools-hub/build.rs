//! Windows：把程序图标和名称嵌入 exe（资源管理器、任务管理器中显示）。
//! macOS 的应用图标由 scripts/bundle-macos.sh 放进 .app。

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // winresource 只是 Windows 主机上的构建依赖，项目也不支持从其他系统交叉编译到 Windows。
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            // 任务管理器把 FileDescription 当作程序名显示，默认是包名 devtools-hub。
            .set("FileDescription", "DevTools Hub")
            .set("ProductName", "DevTools Hub")
            .compile()
            .expect("嵌入 Windows 程序资源失败");
    }
}
