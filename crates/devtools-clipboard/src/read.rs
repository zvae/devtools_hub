//! 平台读取的错误分类：大小超限明确忽略，暂时不可用允许重试。

pub const MAX_ITEMS: usize = 100_000;
pub const MAX_REPRESENTATIONS: usize = 1_000_000;

#[derive(Debug)]
pub enum ReadError {
    Unavailable,
    Changed,
    TooLarge,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unavailable => "剪贴板暂时不可用",
            Self::Changed => "读取时剪贴板已变化",
            Self::TooLarge => "剪贴板内容超过读取预算",
        })
    }
}

impl std::error::Error for ReadError {}
