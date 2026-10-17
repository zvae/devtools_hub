//! NSPasteboard 读写。
//!
//! 与 PasteNow 一致：不依赖剪贴板变化通知（macOS 没有公开的通知），
//! 而是定时比较 `changeCount`，变化后读取所有 item 的所有类型。

use devtools_clipboard::{uti, Representation, Snapshot};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString, NSURL};

/// 系统剪贴板（NSPasteboard）的读写。
pub struct Pasteboard {
    inner: Retained<NSPasteboard>,
}

impl Pasteboard {
    /// 系统通用剪贴板。
    pub fn general() -> Self {
        Self {
            inner: NSPasteboard::generalPasteboard(),
        }
    }

    /// 指定名称的私有剪贴板（测试和调试用，不影响系统剪贴板）。
    pub fn named(name: &str) -> Self {
        Self {
            inner: NSPasteboard::pasteboardWithName(&NSString::from_str(name)),
        }
    }

    /// 新建一个私有剪贴板（测试用，不影响系统剪贴板）。
    #[cfg(test)]
    pub fn unique() -> Self {
        Self {
            inner: NSPasteboard::pasteboardWithUniqueName(),
        }
    }

    /// 剪贴板名，可以传给 `--pasteboard`。
    #[cfg(test)]
    pub fn name(&self) -> String {
        self.inner.name().to_string()
    }

    /// 释放私有剪贴板。
    #[cfg(test)]
    pub fn release(&self) {
        // objc2-app-kit 没有生成这个方法的绑定。
        // SAFETY: `-[NSPasteboard releaseGlobally]` 无参数、无返回值。
        unsafe {
            let _: () = objc2::msg_send![&*self.inner, releaseGlobally];
        }
    }

    /// 剪贴板的修改序号，每次写入都会变；监听线程据此发现新内容。
    pub fn change_count(&self) -> isize {
        self.inner.changeCount()
    }

    /// 只读取类型列表，用于在读取大块数据之前先做隐私标记判断。
    pub fn types(&self) -> Vec<String> {
        autoreleasepool(|_| {
            self.inner
                .pasteboardItems()
                .map(|items| {
                    items
                        .iter()
                        .flat_map(|item| {
                            item.types()
                                .iter()
                                .map(|t| t.to_string())
                                .collect::<Vec<_>>()
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    /// 读取剪贴板上所有条目的所有类型。
    pub fn read(&self) -> Snapshot {
        autoreleasepool(|_| {
            let Some(items) = self.inner.pasteboardItems() else {
                return Snapshot::default();
            };
            let items = items
                .iter()
                .map(|item| {
                    let types: Vec<Retained<NSString>> = item.types().to_vec();
                    let names: Vec<String> = types.iter().map(|t| t.to_string()).collect();
                    let keep_dynamic = names.iter().all(|name| should_skip(name));
                    types
                        .iter()
                        .zip(names)
                        .filter(|(_, name)| keep_dynamic || !should_skip(name))
                        .filter_map(|(ty, name)| {
                            let data = item.dataForType(ty)?.to_vec();
                            let data = if name == uti::FILE_URL {
                                resolve_file_reference(data)
                            } else {
                                data
                            };
                            Some(Representation { uti: name, data })
                        })
                        .collect::<Vec<_>>()
                })
                .filter(|reps| !reps.is_empty())
                .collect();
            Snapshot { items }
        })
    }

    /// 写入快照，返回写入后的 changeCount（调用方据此让监听线程跳过这次自己的写入）。
    pub fn write(&self, snapshot: &Snapshot) -> isize {
        autoreleasepool(|_| {
            let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = snapshot
                .items
                .iter()
                .map(|reps| {
                    let item = NSPasteboardItem::new();
                    for rep in reps {
                        let data = NSData::with_bytes(&rep.data);
                        item.setData_forType(&data, &NSString::from_str(&rep.uti));
                    }
                    ProtocolObject::from_retained(item)
                })
                .collect();
            let count = self.inner.clearContents();
            self.inner
                .writeObjects(&NSArray::from_retained_slice(&items));
            count
        })
    }
}

/// 不保存的类型：
/// - `dyn.*`：系统为旧式类型名动态生成的 UTI，与 public 类型重复；
/// - 文件承诺（promise）类型：读取时会让源应用真正去生成文件；
/// - `CorePasteboardFlavorType`：Carbon 时代的重复表示。
fn should_skip(uti: &str) -> bool {
    uti.starts_with("dyn.")
        || uti.contains("promise")
        || uti.starts_with("CorePasteboardFlavorType")
}

/// Finder 复制文件时写入的是文件引用 URL（`file:///.file/id=卷号.文件号`），这里换成真实路径的 URL：
/// 与 PasteNow 一样显示实际路径，卷重新挂载或重启后也仍然有效。
/// 不是文件引用、或者解析失败（文件已被删除）时保持原样。
fn resolve_file_reference(data: Vec<u8>) -> Vec<u8> {
    let Some(url) = std::str::from_utf8(&data)
        .ok()
        .and_then(|text| NSURL::URLWithString(&NSString::from_str(text.trim_end_matches('\0'))))
    else {
        return data;
    };
    if !url.isFileReferenceURL() {
        return data;
    }
    match url.filePathURL().and_then(|path| path.absoluteString()) {
        Some(resolved) => resolved.to_string().into_bytes(),
        None => data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_round_trip_on_private_pasteboard() {
        let pb = Pasteboard::unique();
        let snapshot = Snapshot {
            items: vec![
                vec![
                    Representation::new(uti::PLAIN_TEXT, "你好，PasteNow".as_bytes()),
                    Representation::new(uti::HTML, b"<b>hi</b>".to_vec()),
                ],
                vec![Representation::new(
                    uti::FILE_URL,
                    b"file:///tmp/a.txt".to_vec(),
                )],
            ],
        };
        let count = pb.write(&snapshot);
        assert_eq!(pb.change_count(), count);
        assert_eq!(pb.read(), snapshot);
        assert!(pb.types().contains(&uti::HTML.to_string()));
        pb.release();
    }

    #[test]
    fn skips_dynamic_types() {
        assert!(should_skip(
            "dyn.ah62d4rv4gu8y6y4grf0gn5xbrzw1gydcr7u1e3cytf2gn"
        ));
        assert!(should_skip("com.apple.pasteboard.promised-file-url"));
        assert!(!should_skip("public.utf8-plain-text"));
    }

    #[test]
    fn resolves_finder_file_references() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("工作簿 1.xlsx");
        std::fs::write(&path, b"x").unwrap();
        let reference = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
            .fileReferenceURL()
            .and_then(|url| url.absoluteString())
            .expect("应能生成文件引用 URL")
            .to_string();
        assert!(reference.starts_with("file:///.file/id="), "{reference}");

        let pb = Pasteboard::unique();
        pb.write(&Snapshot {
            items: vec![vec![Representation::new(
                uti::FILE_URL,
                reference.into_bytes(),
            )]],
        });
        let snapshot = pb.read();
        pb.release();
        let resolved = uti::file_url_to_path(&snapshot.items[0][0].data).expect("应解析出路径");
        // 临时目录在 /var 下，系统解析出的是 /private/var，比较规范化后的路径。
        assert_eq!(
            std::fs::canonicalize(resolved).unwrap(),
            std::fs::canonicalize(&path).unwrap()
        );

        // 普通的路径 URL 和无法解析的引用保持原样。
        let plain = b"file:///tmp/a.txt".to_vec();
        assert_eq!(resolve_file_reference(plain.clone()), plain);
        let missing = b"file:///.file/id=1.1".to_vec();
        assert_eq!(resolve_file_reference(missing.clone()), missing);
    }
}
