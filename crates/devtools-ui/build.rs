fn main() {
    // 以 search_window.slint 作为 UI 聚合入口，实际组件已拆到多个 Slint 文件中。
    slint_build::compile("ui/search_window.slint").expect("failed to compile Slint UI");
}
