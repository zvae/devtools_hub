//! 界面交互与离屏渲染测试，流程与 Base64、UTF-8 工具共用 `support::codec::snapshot_case`。

use super::*;

#[test]
#[ignore = "需要 GPU 离屏渲染，手动运行"]
fn snapshot_url() {
    codec::snapshot_case(
        &SPEC,
        "url",
        "https://example.com/搜索?q=a b&lang=zh",
        "https%3A%2F%2Fexample.com%2F%E6%90%9C%E7%B4%A2%3Fq%3Da%20b%26lang%3Dzh",
        "%E4%B8",
    );
}
