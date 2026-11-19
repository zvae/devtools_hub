//! 快照的二进制编码与压缩。
//!
//! PasteNow 把所有 pasteboard 类型序列化成一个数组后用 LZFSE 压缩
//! （数据库里的 `ZPASTEITEMDATA.ZDATA` 以 `bvx2`/`bvxn` 魔数开头）。
//! 这里用同样的思路：自定义紧凑格式 + zstd。

use anyhow::{bail, ensure, Context, Result};
use std::io::Read;

use crate::model::{Representation, Snapshot};
use crate::read::{MAX_ITEMS, MAX_REPRESENTATIONS};

/// 格式标识与版本号，格式变化时递增版本号。
const MAGIC: &[u8; 4] = b"DTHS";
const VERSION: u8 = 1;
/// zstd 压缩级别：压缩率与速度的折中。
const ZSTD_LEVEL: i32 = 3;
/// 包含类型名和格式元数据的解压预算，独立于压缩文件大小。
pub const MAX_DECODE_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_ENCODED_BYTES: usize = MAX_DECODE_BYTES + 1024 * 1024;

/// 编码格式：
/// `MAGIC | VERSION | u32 item 数 | { u32 表示数 | { u32 uti 长度 | uti | u64 数据长度 | 数据 } }`
pub fn encode(snapshot: &Snapshot) -> Result<Vec<u8>> {
    ensure!(snapshot.items.len() <= MAX_ITEMS, "剪贴板条目数量超过预算");
    let mut raw_len = 9usize + snapshot.items.len() * 4;
    let mut rep_count = 0usize;
    for item in &snapshot.items {
        rep_count = rep_count.checked_add(item.len()).context("表示数量溢出")?;
        ensure!(rep_count <= MAX_REPRESENTATIONS, "剪贴板表示数量超过预算");
        for rep in item {
            raw_len = raw_len
                .checked_add(12)
                .and_then(|len| len.checked_add(rep.uti.len()))
                .and_then(|len| len.checked_add(rep.data.len()))
                .context("编码长度溢出")?;
            ensure!(raw_len <= MAX_DECODE_BYTES, "剪贴板数据超过编码预算");
        }
    }
    let mut raw = Vec::with_capacity(raw_len);
    raw.extend_from_slice(MAGIC);
    raw.push(VERSION);
    raw.extend_from_slice(&(snapshot.items.len() as u32).to_le_bytes());
    for item in &snapshot.items {
        raw.extend_from_slice(&(item.len() as u32).to_le_bytes());
        for rep in item {
            raw.extend_from_slice(&(rep.uti.len() as u32).to_le_bytes());
            raw.extend_from_slice(rep.uti.as_bytes());
            raw.extend_from_slice(&(rep.data.len() as u64).to_le_bytes());
            raw.extend_from_slice(&rep.data);
        }
    }
    ensure!(raw.len() <= MAX_DECODE_BYTES, "剪贴板数据超过编码预算");
    zstd::bulk::compress(&raw, ZSTD_LEVEL).context("压缩剪贴板数据失败")
}

/// 解压并解析 [`encode`] 的输出，数据被截断或格式不对时报错。
pub fn decode(compressed: &[u8]) -> Result<Snapshot> {
    ensure!(
        compressed.len() <= MAX_ENCODED_BYTES,
        "压缩剪贴板数据超过读取预算"
    );
    let raw = decompress(compressed, MAX_DECODE_BYTES)?;
    let mut reader = Reader { buf: &raw, pos: 0 };
    ensure!(reader.take(4)? == MAGIC, "剪贴板数据格式不正确");
    let version = reader.take(1)?[0];
    if version != VERSION {
        bail!("不支持的剪贴板数据版本 {version}");
    }
    let item_count = reader.u32()? as usize;
    ensure!(
        item_count <= MAX_ITEMS && item_count <= reader.remaining() / 4,
        "条目数量超出预算或载荷长度"
    );
    let mut total_reps = 0usize;
    let mut items = Vec::with_capacity(item_count.min(1024));
    for _ in 0..item_count {
        let rep_count = reader.u32()? as usize;
        total_reps = total_reps.checked_add(rep_count).context("表示数量溢出")?;
        ensure!(total_reps <= MAX_REPRESENTATIONS, "剪贴板表示数量超过预算");
        ensure!(rep_count <= reader.remaining() / 12, "表示数量超出载荷长度");
        let mut reps = Vec::with_capacity(rep_count.min(1024));
        for _ in 0..rep_count {
            let uti_len = reader.u32()? as usize;
            let uti =
                String::from_utf8(reader.take(uti_len)?.to_vec()).context("类型名不是 UTF-8")?;
            let data_len = usize::try_from(reader.u64()?).context("数据长度溢出")?;
            let data = reader.take(data_len)?.to_vec();
            reps.push(Representation { uti, data });
        }
        items.push(reps);
    }
    ensure!(reader.pos == raw.len(), "剪贴板数据末尾有多余字节");
    Ok(Snapshot { items })
}

fn decompress(compressed: &[u8], limit: usize) -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    let mut decoder = zstd::stream::read::Decoder::new(compressed)?;
    decoder.window_log_max(27)?;
    decoder
        .take(limit as u64 + 1)
        .read_to_end(&mut raw)
        .context("解压剪贴板数据失败")?;
    ensure!(raw.len() <= limit, "解压剪贴板数据超过预算");
    Ok(raw)
}

/// 按顺序读取字节的游标，越界时报错而不是 panic。
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(len).context("长度溢出")?;
        ensure!(end <= self.buf.len(), "剪贴板数据被截断");
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let snapshot = Snapshot {
            items: vec![
                vec![
                    Representation::new("public.utf8-plain-text", "你好 world".as_bytes()),
                    Representation::new("public.html", b"<b>hi</b>".to_vec()),
                ],
                vec![Representation::new("public.png", vec![0u8; 10_000])],
                vec![],
            ],
        };
        let encoded = encode(&snapshot).unwrap();
        assert!(encoded.len() < 1_000, "零填充数据应被压缩");
        assert_eq!(decode(&encoded).unwrap(), snapshot);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"not zstd").is_err());
        let bad = zstd::bulk::compress(b"DTHS\x01\x05\x00\x00\x00", 1).unwrap();
        assert!(decode(&bad).is_err());
    }
    #[test]
    fn rejects_expansion_beyond_budget() {
        let compressed = zstd::bulk::compress(&vec![0; 4096], 1).unwrap();
        assert!(decompress(&compressed, 128).is_err());
        assert_eq!(decompress(&compressed, 4096).unwrap().len(), 4096);
    }
}
