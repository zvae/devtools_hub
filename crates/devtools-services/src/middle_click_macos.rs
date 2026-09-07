use cocoa::{
    appkit::{NSPasteboard, NSPasteboardItem},
    base::{id, nil},
    foundation::{NSArray, NSAutoreleasePool},
};
use core_foundation::{
    base::{CFType, CFTypeRef, TCFType},
    string::{CFString, CFStringRef},
};
use objc::{
    runtime::{Class, Sel},
    Message,
};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, timeout: f32) -> i32;
    fn AXUIElementGetPid(element: CFTypeRef, pid: *mut i32) -> i32;
}

fn attribute(element: &CFType, name: &str) -> Option<CFType> {
    let name = CFString::new(name);
    let mut value = std::ptr::null();
    let result = unsafe {
        AXUIElementCopyAttributeValue(
            element.as_CFTypeRef(),
            name.as_concrete_TypeRef(),
            &mut value,
        )
    };
    if result != 0 || value.is_null() {
        return None;
    }
    Some(unsafe { CFType::wrap_under_create_rule(value) })
}

fn string_attribute(element: &CFType, name: &str) -> Option<String> {
    let value = attribute(element, name)?;
    value.downcast::<CFString>().map(|value| value.to_string())
}

pub(super) enum Selection {
    Text(String),
    Unavailable,
    Protected,
}

pub(super) fn selected_text() -> Selection {
    read_selection().unwrap_or(Selection::Unavailable)
}

fn read_selection() -> Option<Selection> {
    let system = unsafe { AXUIElementCreateSystemWide() };
    if system.is_null() {
        return None;
    }
    let system = unsafe { CFType::wrap_under_create_rule(system) };
    // A nonresponsive foreground application must not stall the capture worker.
    unsafe {
        AXUIElementSetMessagingTimeout(system.as_CFTypeRef(), 0.2);
    }
    // Query the application, not the system-wide focused element: resolving our own
    // focused view can re-enter AccessKit while its accessibility tree is borrowed.
    let application = attribute(&system, "AXFocusedApplication")?;
    let mut pid = 0;
    if unsafe { AXUIElementGetPid(application.as_CFTypeRef(), &mut pid) } != 0 {
        return None;
    }
    read_external_selection(pid, || read_application_selection(&application))
}

fn read_external_selection(
    pid: i32,
    query: impl FnOnce() -> Option<Selection>,
) -> Option<Selection> {
    if pid <= 0 || pid as u32 == std::process::id() {
        return None;
    }
    query()
}

fn read_application_selection(application: &CFType) -> Option<Selection> {
    unsafe {
        AXUIElementSetMessagingTimeout(application.as_CFTypeRef(), 0.2);
    }
    let focused = attribute(application, "AXFocusedUIElement")?;
    unsafe {
        AXUIElementSetMessagingTimeout(focused.as_CFTypeRef(), 0.2);
    }
    if string_attribute(&focused, "AXSubrole").as_deref() == Some("AXSecureTextField") {
        return Some(Selection::Protected);
    }
    string_attribute(&focused, "AXSelectedText").map(Selection::Text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_queries_our_own_accessibility_tree() {
        let result = read_external_selection(std::process::id() as i32, || {
            panic!("must use clipboard fallback before resolving our focused UI element")
        });
        assert!(result.is_none());
    }

    #[test]
    fn invalid_process_ids_do_not_query_an_accessibility_tree() {
        for pid in [0, -1] {
            assert!(read_external_selection(pid, || panic!("invalid process queried")).is_none());
        }
    }

    #[test]
    fn external_selections_and_protected_fields_are_preserved() {
        let external_pid = if std::process::id() == 1 { 2 } else { 1 };
        assert!(matches!(
            read_external_selection(external_pid, || Some(Selection::Text("selected".into()))),
            Some(Selection::Text(text)) if text == "selected"
        ));
        assert!(matches!(
            read_external_selection(external_pid, || Some(Selection::Protected)),
            Some(Selection::Protected)
        ));
        assert!(read_external_selection(external_pid, || None).is_none());
    }
}

pub(super) fn clipboard_sequence() -> u64 {
    unsafe { NSPasteboard::generalPasteboard(nil).changeCount() as u64 }
}

// All pointers are owned by the capture's autorelease pool. Snapshot each representation
// eagerly: retaining the original pasteboard items alone does not retain their data.
pub(super) struct ClipboardSnapshot {
    items: Vec<id>,
}

impl ClipboardSnapshot {
    pub(super) fn capture() -> Option<Self> {
        unsafe {
            let pasteboard = NSPasteboard::generalPasteboard(nil);
            let items = pasteboard.pasteboardItems();
            let mut saved = Vec::new();
            if items == nil {
                return Some(Self { items: saved });
            }
            for index in 0..items.count() {
                let item = items.objectAtIndex(index);
                let class = Class::get("NSPasteboardItem")?;
                let copy: id = class.send_message(Sel::register("new"), ()).ok()?;
                if copy == nil {
                    return None;
                }
                copy.autorelease();
                let types = NSPasteboardItem::types(item);
                for index in 0..types.count() {
                    let kind = types.objectAtIndex(index);
                    let data = NSPasteboardItem::dataForType(item, kind);
                    if data == nil || !NSPasteboardItem::setData_forType(copy, data, kind) {
                        return None;
                    }
                }
                saved.push(copy);
            }
            Some(Self { items: saved })
        }
    }

    pub(super) fn restore(&self, expected_sequence: u64) {
        unsafe {
            let pasteboard = NSPasteboard::generalPasteboard(nil);
            // Do not overwrite a newer user copy while the capture was running.
            if pasteboard.changeCount() as u64 != expected_sequence {
                return;
            }
            pasteboard.clearContents();
            if !self.items.is_empty() {
                pasteboard.writeObjects(NSArray::arrayWithObjects(nil, &self.items));
            }
        }
    }
}
