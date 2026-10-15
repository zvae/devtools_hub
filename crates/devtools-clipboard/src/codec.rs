//! 快照的二进制编码与压缩。
//!
//! PasteNow 把所有 pasteboard 类型序列化成一个数组后用 LZFSE 压缩
//! （数据库里的 `ZPASTEITEMDATA.ZDATA` 以 `bvx2`/`bvxn` 魔数开头）。
//! 这里用同样的思路：自定义紧凑格式 + zstd。

use anyhow::{bail, ensure, Context, Result};

use crate::model::{Representation, Snapshot};

const MAGIC: &[u8; 4] = b"DTHS";
const VERSION: u8 = 1;
const ZSTD_LEVEL: i32 = 3;

/// 编码格式：
/// `MAGIC | VERSION | u32 item 数 | { u32 表示数 | { u32 uti 长度 | uti | u64 数据长度 | 数据 } }`
pub fn encode(snapshot: &Snapshot) -> Result<Vec<u8>> {
    let mut raw = Vec::with_capacity(snapshot.total_bytes() + 64);
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
    zstd::bulk::compress(&raw, ZSTD_LEVEL).context("压缩剪贴板数据失败")
}

pub fn decode(compressed: &[u8]) -> Result<Snapshot> {
    let raw = zstd::stream::decode_all(compressed).context("解压剪贴板数据失败")?;
    let mut reader = Reader { buf: &raw, pos: 0 };
    ensure!(reader.take(4)? == MAGIC, "剪贴板数据格式不正确");
    let version = reader.take(1)?[0];
    if version != VERSION {
        bail!("不支持的剪贴板数据版本 {version}");
    }
    let item_count = reader.u32()? as usize;
    let mut items = Vec::with_capacity(item_count.min(1024));
    for _ in 0..item_count {
        let rep_count = reader.u32()? as usize;
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

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
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
}
