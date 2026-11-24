//! 随机密码生成，与 JSON、时间戳一样独立于界面与平台。
//!
//! 使用系统安全随机源；拒绝采样消除取模偏差。整串候选不满足字符类型时重新生成，
//! 因此所有满足约束的密码等概率，不用在固定位置补字符。随机源失败时直接报错，不降级。

use std::fmt;

pub const MIN_LENGTH: usize = 8;
pub const MAX_LENGTH: usize = 100;
const LOWER: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const UPPER: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &[u8] = b"0123456789";
const SYMBOLS: &[u8] = b"!@#$%^&*()-_=+[]{};:,.?";

/// 仅提供随机模式，始终含大小写字母，可额外包含数字及符号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub length: usize,
    pub numbers: bool,
    pub symbols: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            length: 12,
            numbers: true,
            symbols: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidLength,
    RandomUnavailable,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidLength => "密码长度必须在 8–100 位之间",
            Self::RandomUnavailable => "无法访问系统安全随机源，请重试",
        })
    }
}

impl std::error::Error for Error {}

/// 生成符合配置的密码；开启的字符类型至少出现一次，不记录或保存结果。
pub fn generate(options: Options) -> Result<String, Error> {
    generate_with(options, |buf| {
        getrandom::fill(buf).map_err(|_| Error::RandomUnavailable)
    })
}

fn generate_with(
    options: Options,
    mut fill: impl FnMut(&mut [u8]) -> Result<(), Error>,
) -> Result<String, Error> {
    if !(MIN_LENGTH..=MAX_LENGTH).contains(&options.length) {
        return Err(Error::InvalidLength);
    }
    let mut groups = vec![LOWER, UPPER];
    if options.numbers {
        groups.push(DIGITS);
    }
    if options.symbols {
        groups.push(SYMBOLS);
    }
    let alphabet: Vec<u8> = groups
        .iter()
        .flat_map(|group| group.iter().copied())
        .collect();
    let mut random = [0u8; 128];
    let mut cursor = random.len();
    loop {
        let mut password = Vec::with_capacity(options.length);
        while password.len() < options.length {
            if cursor == random.len() {
                fill(&mut random)?;
                cursor = 0;
            }
            let byte = random[cursor];
            cursor += 1;
            if let Some(index) = uniform_index(byte, alphabet.len()) {
                password.push(alphabet[index]);
            }
        }
        if groups
            .iter()
            .all(|group| password.iter().any(|byte| group.contains(byte)))
        {
            // 字符表只含 ASCII，转换不可能失败。
            return Ok(String::from_utf8(password).expect("密码字符表必须为 ASCII"));
        }
    }
}

/// 舍弃不能均分到字符表的尾部区间，保留区间内每个下标的原像数量完全相同。
fn uniform_index(byte: u8, count: usize) -> Option<usize> {
    ((byte as usize) < 256 / count * count).then_some(byte as usize % count)
}

#[cfg(test)]
mod tests;
