//! NSPasteboard 读写。
//!
//! 与 PasteNow 一致：不依赖剪贴板变化通知（macOS 没有公开的通知），
//! 而是定时比较 `changeCount`，变化后读取所有 item 的所有类型。

use devtools_clipboard::{Representation, Snapshot};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardWriting};
use objc2_foundation::{NSArray, NSData, NSString};

pub struct Pasteboard {
    inner: Retained<NSPasteboard>,
}

impl Pasteboard {
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

    #[cfg(test)]
    pub fn unique() -> Self {
        Self {
            inner: NSPasteboard::pasteboardWithUniqueName(),
        }
    }

    #[cfg(test)]
    pub fn name(&self) -> String {
        self.inner.name().to_string()
    }

    #[cfg(test)]
    pub fn release(&self) {
        // objc2-app-kit 没有生成这个方法的绑定。
        // SAFETY: `-[NSPasteboard releaseGlobally]` 无参数、无返回值。
        unsafe {
            let _: () = objc2::msg_send![&*self.inner, releaseGlobally];
        }
    }

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
                            let data = item.dataForType(ty)?;
                            Some(Representation {
                                uti: name,
                                data: data.to_vec(),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .filter(|reps| !reps.is_empty())
                .collect();
            Snapshot { items }
        })
    }

    /// 写入快照，返回写入后的 changeCount。
    ///
    /// `before_clear` 在清空剪贴板前被调用，参数是预期的新 changeCount，
    /// 监听线程据此跳过这次由我们自己触发的变化。
    pub fn write(&self, snapshot: &Snapshot, before_clear: impl FnOnce(isize)) -> isize {
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
            before_clear(self.inner.changeCount() + 1);
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

#[cfg(test)]
mod tests {
    use super::*;
    use devtools_clipboard::uti;

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
        let mut expected = 0;
        let count = pb.write(&snapshot, |next| expected = next);
        assert_eq!(count, expected);
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
}
