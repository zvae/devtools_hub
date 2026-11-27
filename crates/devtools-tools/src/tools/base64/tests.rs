//! 界面交互与离屏渲染测试，流程与 URL、UTF-8 工具共用 `support::codec::snapshot_case`。

use super::*;

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_base64() {
    codec::snapshot_case(
        &SPEC,
        "base64",
        "你好，DevTools Hub",
        "5L2g5aW977yMRGV2VG9vbHMgSHVi",
        "@@@@",
    );
}
