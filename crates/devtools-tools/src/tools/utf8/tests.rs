//! 界面交互与离屏渲染测试，流程与 Base64、URL 工具共用 `support::codec::snapshot_case`。

use super::*;

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_utf8() {
    codec::snapshot_case(
        &SPEC,
        "utf8",
        "中文",
        "\\xE4\\xB8\\xAD\\xE6\\x96\\x87",
        "\\xE4\\xB8",
    );
}
