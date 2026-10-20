//! 自绘的右键菜单（Windows）。系统菜单的样式与面板不搭，这里用分层窗口画一个与 macOS `NSMenu`
//! 相同样式的菜单（排布与绘制见 `platform::drawn_menu`），文字用 GDI 画。
//!
//! 用法与 `TrackPopupMenu` 一样：[`popup`] 在鼠标位置弹出菜单，自己跑消息循环，菜单关闭后才返回选中项。
//! 菜单窗口不会被激活（`WS_EX_NOACTIVATE`），面板一直是前台窗口，键盘消息仍发给面板，
//! 在消息循环里截下来交给菜单。点到菜单外面、按 esc、面板不再是前台窗口时关闭菜单。

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::Once;

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, DrawTextW, GdiFlush,
    GetDC, ReleaseDC, SelectObject, SetBkMode, SetTextColor, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
    DEFAULT_CHARSET, DEFAULT_PITCH, DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_END_ELLIPSIS,
    DT_LEFT, DT_NOPREFIX, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, FW_NORMAL, HBITMAP,
    HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DOWN, VK_END, VK_ESCAPE, VK_F10, VK_HOME,
    VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT, VK_SPACE, VK_UP,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow,
    GetMessageW, IsWindowVisible, KillTimer, LoadCursorW, PostQuitMessage, RegisterClassExW,
    SetTimer, ShowWindow, TranslateMessage, UpdateLayeredWindow, IDC_ARROW, MA_NOACTIVATE, MSG,
    SW_SHOWNOACTIVATE, ULW_ALPHA, WM_CHAR, WM_DEADCHAR, WM_KEYDOWN, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN,
    WM_NCMBUTTONDOWN, WM_NCMOUSEMOVE, WM_NCRBUTTONDOWN, WM_NCXBUTTONDOWN, WM_RBUTTONDBLCLK,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSCHAR, WM_SYSDEADCHAR, WM_SYSKEYDOWN, WM_TIMER,
    WM_XBUTTONDBLCLK, WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
};

use super::panel::{cursor_position, work_area_at};
use super::wide;
use crate::platform::drawn_menu::{self, Canvas, Colors, Frame, Layout, Node, Rect, Rgba};

/// 菜单文字的字号（逻辑像素），与面板正文一致。
const TEXT_SIZE: f32 = 13.0;
/// 中文用微软雅黑 UI，西文字形与 Segoe UI 相同。
const FONT: &str = "Microsoft YaHei UI";
const CLASS: &str = "DevToolsHubMenu";
/// 检查面板是否还是前台窗口的间隔（毫秒）。
const CHECK_INTERVAL: u32 = 100;
/// `WM_MOUSELEAVE` 定义在 `Win32_UI_Controls` 里，为一个常量不必多开一个特性。
const WM_MOUSELEAVE: u32 = 0x02A3;

/// 在鼠标位置弹出菜单，菜单关闭后才返回；返回选中菜单项的 `id`。面板窗口不存在时不弹出。
pub fn popup(nodes: &[Node], colors: &Colors) -> Option<usize> {
    popup_owned(super::panel::window(), nodes, colors)
}

/// 同 [`popup`]，菜单属于指定的窗口（贴图窗口的右键菜单）。窗口为空时不弹出。
pub fn popup_owned(owner: HWND, nodes: &[Node], colors: &Colors) -> Option<usize> {
    if owner.is_null() || nodes.is_empty() {
        return None;
    }
    // SAFETY: 在主线程（面板、贴图窗口所在线程）调用；窗口、DC 等资源都由 `Session` 创建并在返回前释放。
    unsafe { Session::run(owner, nodes, *colors) }
}

/// 一次弹出：各层菜单窗口与消息循环的状态。
struct Session<'a> {
    owner: HWND,
    colors: Colors,
    scale: f32,
    /// 打开的各层菜单，第 0 层是根菜单。字段按声明顺序释放：窗口要在字体之前释放。
    levels: Vec<Level<'a>>,
    font: Font,
    /// 弹出菜单时的前台窗口；前台换了说明用户点到了别的程序。
    foreground: HWND,
}

/// 一层菜单。
struct Level<'a> {
    nodes: &'a [Node],
    window: MenuWindow,
    /// 高亮的菜单项：鼠标悬停、键盘选中，或者展开了下一层子菜单的那一项。
    highlighted: Option<usize>,
}

/// 一条消息的处理结果。
enum Outcome {
    /// 交给原来的窗口处理。
    Dispatch,
    /// 菜单处理了，不再分发。
    Consume,
    /// 关闭菜单，返回选中项。
    Close(Option<usize>),
}

impl<'a> Session<'a> {
    unsafe fn run(owner: HWND, nodes: &'a [Node], colors: Colors) -> Option<usize> {
        register_class();
        let dpi = GetDpiForWindow(owner);
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        let font = Font::new(scale)?;
        let mut session = Session {
            owner,
            colors,
            scale,
            levels: Vec::new(),
            font,
            foreground: GetForegroundWindow(),
        };
        let cursor = cursor_position();
        let layout = session.layout(nodes);
        let size = (layout.body.width(), layout.body.height());
        let origin =
            drawn_menu::place_root((cursor.x, cursor.y), size, work_rect(work_area_at(cursor)));
        if !session.open(nodes, layout, origin) {
            return None;
        }
        let timer = SetTimer(null_mut(), 0, CHECK_INTERVAL, None);
        let chosen = session.message_loop(timer);
        KillTimer(null_mut(), timer);
        chosen
    }

    fn layout(&self, nodes: &[Node]) -> Layout {
        Layout::new(nodes, self.scale, |text| self.font.measure(text))
    }

    /// 打开一层菜单，底板左上角放在屏幕坐标 `origin`。
    unsafe fn open(&mut self, nodes: &'a [Node], layout: Layout, origin: (i32, i32)) -> bool {
        let position = POINT {
            x: origin.0 - layout.body.left,
            y: origin.1 - layout.body.top,
        };
        let Some(window) = MenuWindow::new(self.owner, layout, position, &self.font, &self.colors)
        else {
            return false;
        };
        window.render(nodes, None, &self.colors);
        ShowWindow(window.hwnd, SW_SHOWNOACTIVATE);
        self.levels.push(Level {
            nodes,
            window,
            highlighted: None,
        });
        true
    }

    /// 展开第 `level` 层第 `row` 项的子菜单，放在这一项旁边。
    unsafe fn open_submenu(&mut self, level: usize, row: usize) {
        let nodes = self.levels[level].nodes;
        let Node::Submenu { children, .. } = &nodes[row] else {
            return;
        };
        if children.is_empty() {
            return;
        }
        let layout = self.layout(children);
        let parent = &self.levels[level].window;
        let parent_body = parent.screen(parent.layout.body);
        let row_top = parent.screen(parent.layout.rows[row]).top;
        let anchor = POINT {
            x: parent_body.right,
            y: row_top,
        };
        let origin = drawn_menu::place_submenu(
            parent_body,
            row_top,
            layout.margin(),
            (layout.body.width(), layout.body.height()),
            work_rect(work_area_at(anchor)),
        );
        self.open(children, layout, origin);
    }

    unsafe fn message_loop(&mut self, timer: usize) -> Option<usize> {
        let mut msg: MSG = zeroed();
        loop {
            match GetMessageW(&mut msg, null_mut(), 0, 0) {
                // 收到 WM_QUIT：放回队列交给外层的消息循环，关闭菜单。
                0 => {
                    PostQuitMessage(msg.wParam as i32);
                    return None;
                }
                -1 => return None,
                _ => {}
            }
            match self.handle(&msg, timer) {
                Outcome::Dispatch => {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                Outcome::Consume => {}
                Outcome::Close(chosen) => return chosen,
            }
        }
    }

    unsafe fn handle(&mut self, msg: &MSG, timer: usize) -> Outcome {
        match msg.message {
            WM_TIMER if msg.hwnd.is_null() && msg.wParam == timer => {
                // 用户点了别的程序（面板不再是前台窗口），或者面板被隐藏了。
                if GetForegroundWindow() != self.foreground || IsWindowVisible(self.owner) == 0 {
                    Outcome::Close(None)
                } else {
                    Outcome::Consume
                }
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => self.key(msg.wParam as u16),
            WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR => Outcome::Consume,
            WM_MOUSEMOVE | WM_NCMOUSEMOVE => {
                self.hover(msg.hwnd, msg.pt);
                Outcome::Consume
            }
            WM_MOUSELEAVE => {
                if self.level_of(msg.hwnd) == Some(self.levels.len() - 1) {
                    // 鼠标离开了最里层的菜单：取消高亮（外层保留展开子菜单的那一项）。
                    self.set_highlight(self.levels.len() - 1, None);
                }
                Outcome::Consume
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
            | WM_LBUTTONDBLCLK | WM_RBUTTONDBLCLK | WM_MBUTTONDBLCLK | WM_XBUTTONDBLCLK
            | WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN | WM_NCMBUTTONDOWN | WM_NCXBUTTONDOWN
            | WM_NCLBUTTONDBLCLK => {
                // 在菜单上按下时等松开再选；点到菜单外面（包括菜单的阴影）就关闭菜单，这次点击不再传下去。
                if self.on_body(msg.hwnd, msg.pt) {
                    Outcome::Consume
                } else {
                    Outcome::Close(None)
                }
            }
            WM_LBUTTONUP | WM_RBUTTONUP => self.release(msg.hwnd, msg.pt),
            WM_MBUTTONUP | WM_XBUTTONUP | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => Outcome::Consume,
            _ => Outcome::Dispatch,
        }
    }

    /// `hwnd` 是第几层菜单的窗口。
    fn level_of(&self, hwnd: HWND) -> Option<usize> {
        self.levels
            .iter()
            .position(|level| level.window.hwnd == hwnd)
    }

    /// 屏幕坐标 `point` 在第几层菜单的哪一项上（`hwnd` 是收到鼠标消息的窗口）。
    fn hit(&self, hwnd: HWND, point: POINT) -> Option<(usize, Option<usize>)> {
        let level = self.level_of(hwnd)?;
        let entry = &self.levels[level];
        let window = &entry.window;
        let row = window.layout.hit(
            entry.nodes,
            point.x - window.position.x,
            point.y - window.position.y,
        );
        Some((level, row))
    }

    /// 屏幕坐标 `point` 在菜单底板上（不算阴影）。
    fn on_body(&self, hwnd: HWND, point: POINT) -> bool {
        self.level_of(hwnd).is_some_and(|level| {
            let window = &self.levels[level].window;
            window.screen(window.layout.body).contains(point.x, point.y)
        })
    }

    unsafe fn hover(&mut self, hwnd: HWND, point: POINT) {
        let deepest = self.levels.len() - 1;
        match self.hit(hwnd, point) {
            Some((level, Some(row))) => {
                track_leave(hwnd);
                self.highlight(level, row);
            }
            Some((level, None)) => {
                track_leave(hwnd);
                // 在分隔线、留白或阴影上：展开了子菜单的那一层保留高亮。
                if level == deepest {
                    self.set_highlight(level, None);
                }
            }
            // 鼠标在菜单外面（面板上）。
            None => self.set_highlight(deepest, None),
        }
    }

    /// 高亮第 `level` 层的第 `row` 项：关掉更深层里不是由它展开的子菜单，子菜单项悬停时展开。
    unsafe fn highlight(&mut self, level: usize, row: usize) {
        if self.levels.len() > level + 1 && self.levels[level].highlighted != Some(row) {
            self.levels.truncate(level + 1);
        }
        self.set_highlight(level, Some(row));
        if self.levels.len() == level + 1 {
            self.open_submenu(level, row);
        }
    }

    unsafe fn set_highlight(&mut self, level: usize, row: Option<usize>) {
        let entry = &mut self.levels[level];
        if entry.highlighted != row {
            entry.highlighted = row;
            entry.window.render(entry.nodes, row, &self.colors);
        }
    }

    /// 松开鼠标：选中菜单项，或者展开子菜单。
    unsafe fn release(&mut self, hwnd: HWND, point: POINT) -> Outcome {
        let Some((level, Some(row))) = self.hit(hwnd, point) else {
            return Outcome::Consume;
        };
        let nodes = self.levels[level].nodes;
        match &nodes[row] {
            Node::Item { id, .. } => Outcome::Close(Some(*id)),
            _ => {
                self.highlight(level, row);
                Outcome::Consume
            }
        }
    }

    /// 键盘操作最里层的菜单：上下移动，→ 展开子菜单，← 收起子菜单，回车选中，esc 关闭。
    unsafe fn key(&mut self, key: u16) -> Outcome {
        let deepest = self.levels.len() - 1;
        let nodes = self.levels[deepest].nodes;
        let current = self.levels[deepest].highlighted;
        match key {
            VK_ESCAPE | VK_LEFT if deepest > 0 => self.levels.truncate(deepest),
            VK_ESCAPE | VK_MENU | VK_F10 => return Outcome::Close(None),
            VK_UP | VK_DOWN | VK_HOME | VK_END => {
                let next = match key {
                    VK_UP => drawn_menu::step(nodes, current, false),
                    VK_DOWN => drawn_menu::step(nodes, current, true),
                    VK_HOME => drawn_menu::step(nodes, None, true),
                    _ => drawn_menu::step(nodes, None, false),
                };
                if next.is_some() {
                    self.set_highlight(deepest, next);
                }
            }
            VK_RIGHT | VK_RETURN | VK_SPACE => match current.map(|row| (row, &nodes[row])) {
                Some((row, Node::Submenu { children, .. })) => {
                    self.open_submenu(deepest, row);
                    if self.levels.len() > deepest + 1 {
                        self.set_highlight(deepest + 1, drawn_menu::step(children, None, true));
                    }
                }
                Some((_, Node::Item { id, .. })) if key != VK_RIGHT => {
                    return Outcome::Close(Some(*id));
                }
                _ => {}
            },
            _ => {}
        }
        Outcome::Consume
    }
}

/// 一层菜单的窗口与绘制用的位图。
struct MenuWindow {
    hwnd: HWND,
    /// 选入了 `bitmap` 与字体的内存 DC。
    dc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    /// `bitmap` 的像素（32 位，自上而下）。
    bits: *mut u32,
    layout: Layout,
    frame: Frame,
    /// 窗口左上角的屏幕坐标。
    position: POINT,
}

impl MenuWindow {
    unsafe fn new(
        owner: HWND,
        layout: Layout,
        position: POINT,
        font: &Font,
        colors: &Colors,
    ) -> Option<Self> {
        let class = wide(CLASS);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            null(),
            WS_POPUP,
            position.x,
            position.y,
            layout.width,
            layout.height,
            owner,
            null_mut(),
            GetModuleHandleW(null()),
            null(),
        );
        if hwnd.is_null() {
            tracing::warn!("创建菜单窗口失败");
            return None;
        }
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = layout.width;
        // 高度为负：像素自上而下排列。
        info.bmiHeader.biHeight = -layout.height;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let dc = CreateCompatibleDC(null_mut());
        let mut bits = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            tracing::warn!("创建菜单位图失败");
            DeleteDC(dc);
            DestroyWindow(hwnd);
            return None;
        }
        let old_bitmap = SelectObject(dc, bitmap);
        SelectObject(dc, font.handle);
        SetBkMode(dc, TRANSPARENT as i32);
        let frame = Frame::new(&layout, colors);
        Some(Self {
            hwnd,
            dc,
            bitmap,
            old_bitmap,
            bits: bits.cast(),
            layout,
            frame,
            position,
        })
    }

    /// 窗口坐标的矩形 → 屏幕坐标。
    fn screen(&self, rect: Rect) -> Rect {
        rect.offset(self.position.x, self.position.y)
    }

    /// 重画菜单并更新到屏幕上。
    unsafe fn render(&self, nodes: &[Node], highlighted: Option<usize>, colors: &Colors) {
        let (width, height) = (self.layout.width, self.layout.height);
        let len = (width * height) as usize;
        // CPU 直接读写位图前都要等 GDI 把排队的绘制做完；GDI 画文字时不持有像素的引用。
        GdiFlush();
        let mut canvas = Canvas {
            pixels: std::slice::from_raw_parts_mut(self.bits, len),
            width,
            height,
        };
        drawn_menu::paint(&mut canvas, &self.layout, nodes, highlighted, colors);
        self.draw_text(nodes, highlighted, colors);
        GdiFlush();
        self.frame
            .apply(std::slice::from_raw_parts_mut(self.bits, len), colors);

        let screen = GetDC(null_mut());
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let source = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        UpdateLayeredWindow(
            self.hwnd,
            screen,
            &self.position,
            &size,
            self.dc,
            &source,
            0,
            &blend,
            ULW_ALPHA,
        );
        ReleaseDC(null_mut(), screen);
    }

    /// 菜单项名字左对齐，过长时显示省略号；快捷键右对齐。
    unsafe fn draw_text(&self, nodes: &[Node], highlighted: Option<usize>, colors: &Colors) {
        let layout = &self.layout;
        for (index, (node, row)) in nodes.iter().zip(&layout.rows).enumerate() {
            let (label, shortcut) = match node {
                Node::Item {
                    label, shortcut, ..
                } => (label, shortcut.as_deref()),
                Node::Submenu { label, .. } => (label, None),
                Node::Separator => continue,
            };
            let (text_color, shortcut_color) = colors.text(highlighted == Some(index));
            let label_rect = RECT {
                left: layout.label_left,
                top: row.top,
                right: layout.label_left + layout.label_width,
                bottom: row.bottom,
            };
            draw_text(
                self.dc,
                label,
                label_rect,
                text_color,
                DT_LEFT | DT_END_ELLIPSIS,
            );
            if let Some(shortcut) = shortcut {
                let shortcut_rect = RECT {
                    left: layout.label_left,
                    top: row.top,
                    right: layout.text_right,
                    bottom: row.bottom,
                };
                draw_text(self.dc, shortcut, shortcut_rect, shortcut_color, DT_RIGHT);
            }
        }
    }
}

impl Drop for MenuWindow {
    fn drop(&mut self) {
        // SAFETY: 句柄都由 `MenuWindow::new` 创建，只在这里释放一次。
        unsafe {
            DestroyWindow(self.hwnd);
            SelectObject(self.dc, self.old_bitmap);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

/// 菜单字体，以及量文字宽度用的 DC。
struct Font {
    handle: HFONT,
    dc: HDC,
}

impl Font {
    unsafe fn new(scale: f32) -> Option<Self> {
        let face = wide(FONT);
        // 高度为负：按字号（不含行距）取字体。
        let handle = CreateFontW(
            -(TEXT_SIZE * scale).round() as i32,
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            face.as_ptr(),
        );
        if handle.is_null() {
            tracing::warn!("创建菜单字体失败");
            return None;
        }
        let dc = CreateCompatibleDC(null_mut());
        SelectObject(dc, handle);
        Some(Self { handle, dc })
    }

    /// 文字的宽度（物理像素）。与绘制一样用 `DrawTextW` 量，缺字时的字体回退也算在内。
    fn measure(&self, text: &str) -> i32 {
        let text: Vec<u16> = text.encode_utf16().collect();
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        // SAFETY: dc 由 `Font::new` 创建；文字长度与缓冲区一致。
        unsafe {
            DrawTextW(
                self.dc,
                text.as_ptr(),
                text.len() as i32,
                &mut rect,
                DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
        rect.right - rect.left
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: 先删除选入了字体的 DC，再删除字体。
        unsafe {
            DeleteDC(self.dc);
            DeleteObject(self.handle);
        }
    }
}

/// 在 `rect` 里画一行竖直居中的文字。
unsafe fn draw_text(dc: HDC, text: &str, mut rect: RECT, color: Rgba, align: DRAW_TEXT_FORMAT) {
    let text: Vec<u16> = text.encode_utf16().collect();
    SetTextColor(dc, colorref(color));
    DrawTextW(
        dc,
        text.as_ptr(),
        text.len() as i32,
        &mut rect,
        align | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
}

fn colorref([r, g, b, _]: Rgba) -> COLORREF {
    u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16
}

fn work_rect(rect: RECT) -> Rect {
    Rect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

/// 鼠标离开窗口时收到一次 `WM_MOUSELEAVE`（每次移进来都要重新登记）。
unsafe fn track_leave(hwnd: HWND) {
    let mut track = TRACKMOUSEEVENT {
        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
        dwFlags: TME_LEAVE,
        hwndTrack: hwnd,
        dwHoverTime: 0,
    };
    TrackMouseEvent(&mut track);
}

fn register_class() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let class = wide(CLASS);
        // SAFETY: 结构体全部字段都是整数或指针，置零后再填需要的字段。
        unsafe {
            let mut info: WNDCLASSEXW = zeroed();
            info.cbSize = size_of::<WNDCLASSEXW>() as u32;
            info.lpfnWndProc = Some(window_proc);
            info.hInstance = GetModuleHandleW(null());
            info.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
            info.lpszClassName = class.as_ptr();
            if RegisterClassExW(&info) == 0 {
                tracing::warn!("注册菜单窗口类失败");
            }
        }
    });
}

/// 菜单窗口的窗口过程：点击时不激活（焦点留在面板上），其余交给系统默认处理。
/// 鼠标与键盘消息在 [`Session::message_loop`] 里处理，分层窗口也不需要处理 `WM_PAINT`。
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_MOUSEACTIVATE {
        return MA_NOACTIVATE as LRESULT;
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}
