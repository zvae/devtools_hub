//! 临时：打印实际装配的字体族（人工检查用）。
#[test]
fn print_font_families() {
    let defs = crate::fonts::definitions();
    for (family, faces) in &defs.families {
        eprintln!("{family:?}: {faces:?}");
    }
}
