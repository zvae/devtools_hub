//! 模拟 ⌘V 粘贴、⌘C 复制。
//!
//! PasteNow 导入了 `CGEventSourceCreate` / `CGEventCreateKeyboardEvent` /
//! `CGEventSetFlags` / `CGEventPost`，并用 `AXIsProcessTrustedWithOptions` 申请辅助功能权限；
//! 用 `TISCopyCurrentASCIICapableKeyboardLayoutInputSource` + `UCKeyTranslate` 找到当前键盘布局下
//! 字母 V 的键码（日志 “Simulate paste item: no avaiable key code”）。这里是同样的实现。
//! 鼠标中键快捷面板读取选中文本时，与 uTools 一样模拟 ⌘C（`simulateKeyboardTap("c","command")`）。

use std::ffi::c_void;
use std::ptr;

/// Core Foundation 对象的指针（这里不区分具体类型）。
type CFTypeRef = *const c_void;

// 用到的系统函数与常量，名字与 Apple 头文件中的一致。
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
    static kAXTrustedCheckOptionPrompt: CFTypeRef;

    fn CGEventSourceCreate(state_id: i32) -> CFTypeRef;
    fn CGEventCreateKeyboardEvent(source: CFTypeRef, keycode: u16, key_down: bool) -> CFTypeRef;
    fn CGEventSetFlags(event: CFTypeRef, flags: u64);
    fn CGEventPost(tap: u32, event: CFTypeRef);
}

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> CFTypeRef;
    fn TISGetInputSourceProperty(source: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    static kTISPropertyUnicodeKeyLayoutData: CFTypeRef;
    fn LMGetKbdType() -> u8;
    #[allow(clippy::too_many_arguments)]
    fn UCKeyTranslate(
        layout: *const c_void,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        key_translate_options: u32,
        dead_key_state: *mut u32,
        max_string_length: usize,
        actual_string_length: *mut usize,
        unicode_string: *mut u16,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: isize,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    static kCFBooleanTrue: CFTypeRef;
}

// 常量取值见 CGEventSource.h、CGEventTypes.h 与 UnicodeUtilities.h。
const K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION: i32 = 0;
const K_CG_SESSION_EVENT_TAP: u32 = 1;
const K_CG_EVENT_FLAG_MASK_COMMAND: u64 = 0x0010_0000;
const K_UC_KEY_ACTION_DISPLAY: u16 = 3;
const K_UC_KEY_TRANSLATE_NO_DEAD_KEYS: u32 = 1;
/// ANSI 布局下 V、C 的键码（kVK_ANSI_V、kVK_ANSI_C）。
const KVK_ANSI_V: u16 = 9;
const KVK_ANSI_C: u16 = 8;

/// 是否已获得“辅助功能”权限；`prompt` 为 true 时若未授权会弹出系统授权提示。
pub fn accessibility_trusted(prompt: bool) -> bool {
    unsafe {
        if !prompt {
            return AXIsProcessTrustedWithOptions(ptr::null());
        }
        let keys = [kAXTrustedCheckOptionPrompt];
        let values = [kCFBooleanTrue];
        let options = CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        );
        let trusted = AXIsProcessTrustedWithOptions(options);
        if !options.is_null() {
            CFRelease(options);
        }
        trusted
    }
}

/// 打开“系统设置 › 隐私与安全性 › 辅助功能”。
pub fn open_accessibility_settings() {
    super::workspace::open_url(
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    );
}

/// 当前键盘布局下产生字母 `v` 的键码。
///
/// 必须在主线程调用：新版 macOS 上 TIS 系列函数在后台线程调用会触发断言。
pub fn paste_keycode() -> u16 {
    keycode_for(b'v', KVK_ANSI_V)
}

/// 当前键盘布局下产生字母 `c` 的键码（模拟复制以读取选中的内容）。必须在主线程调用。
pub fn copy_keycode() -> u16 {
    keycode_for(b'c', KVK_ANSI_C)
}

/// 在当前 ASCII 键盘布局中逐个键码试出产生 `letter` 的那个（例如 Dvorak 布局下 V、C 不在 ANSI 布局的位置）；
/// 找不到时用 ANSI 布局的 `fallback`。
fn keycode_for(letter: u8, fallback: u16) -> u16 {
    unsafe {
        let source = TISCopyCurrentASCIICapableKeyboardLayoutInputSource();
        if source.is_null() {
            return fallback;
        }
        let layout_data = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
        let found = if layout_data.is_null() {
            None
        } else {
            let layout = CFDataGetBytePtr(layout_data) as *const c_void;
            let keyboard_type = LMGetKbdType() as u32;
            (0u16..128).find(|&code| {
                let mut dead_keys = 0u32;
                let mut length = 0usize;
                let mut chars = [0u16; 4];
                let status = UCKeyTranslate(
                    layout,
                    code,
                    K_UC_KEY_ACTION_DISPLAY,
                    0,
                    keyboard_type,
                    K_UC_KEY_TRANSLATE_NO_DEAD_KEYS,
                    &mut dead_keys,
                    chars.len(),
                    &mut length,
                    chars.as_mut_ptr(),
                );
                status == 0 && length == 1 && chars[0] == u16::from(letter)
            })
        };
        CFRelease(source);
        found.unwrap_or(fallback)
    }
}

/// 向当前前台应用发送 ⌘V。可以在任意线程调用。
pub fn post_paste(keycode: u16) {
    post_command(keycode);
}

/// 向当前前台应用发送 ⌘C。可以在任意线程调用。
pub fn post_copy(keycode: u16) {
    post_command(keycode);
}

/// 发送 ⌘ + `keycode`。
fn post_command(keycode: u16) {
    unsafe {
        let source = CGEventSourceCreate(K_CG_EVENT_SOURCE_STATE_COMBINED_SESSION);
        for key_down in [true, false] {
            let event = CGEventCreateKeyboardEvent(source, keycode, key_down);
            if event.is_null() {
                continue;
            }
            // 显式设置标志位，避免用户仍按着的 ⇧ 等修饰键混进来变成 ⇧⌘V。
            CGEventSetFlags(event, K_CG_EVENT_FLAG_MASK_COMMAND);
            CGEventPost(K_CG_SESSION_EVENT_TAP, event);
            CFRelease(event);
        }
        if !source.is_null() {
            CFRelease(source);
        }
    }
}
