//! 截屏与贴图中与平台、界面无关的部分（参照 Snipaste，分析见 `docs/Snipaste截图贴图实现分析.md`）：
//!
//! - [`Capture`]：一次截屏得到的显示器图像，以及截屏时屏幕上各窗口的位置（换算成图像中的像素）；
//! - 选区：两点围成的区域、八个控制点的命中与拖动、移动，全部取整到整像素并限制在图像内；
//! - 图片：裁剪、旋转、翻转、编码成 PNG，以及从剪贴板快照中取出图片；
//! - [`annotation`]：标注（矩形 / 椭圆、箭头、画笔、记号笔、马赛克、文字、橡皮擦，撤销重做）。
//!
//! 截屏遮罩（`ui::snip`）和贴图窗口（`ui::pin`）只负责绘制和交互，计算都在这里，便于测试。

pub mod annotation;
pub mod scroll;

use std::io::Cursor;

use anyhow::{Context, Result};
use devtools_clipboard::{uti, Representation, Snapshot};
use egui::{pos2, vec2, Pos2, Rect, Vec2};
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ImageReader, RgbaImage};

/// 截屏时鼠标所在的显示器（由平台层给出）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    /// 平台的显示器编号（macOS 的 `CGDirectDisplayID`，Windows 不用）。
    pub id: u32,
    /// 在桌面上的位置与大小 `[x, y, 宽, 高]`，单位是平台的全局坐标：
    /// macOS 为点（主显示器左上角为原点，与 `CGDisplayBounds` 一致），Windows 为物理像素。
    pub frame: [f64; 4],
    /// 同一区域在 egui 中的逻辑坐标（点），用来创建窗口。
    pub logical: Rect,
    /// 每个逻辑点对应的物理像素数（Retina 屏为 2）。
    pub scale: f32,
}

impl Display {
    /// 全局坐标中的点换算成逻辑坐标。
    pub fn to_logical(self, point: [f64; 2]) -> Pos2 {
        let [x, y, width, height] = self.frame;
        self.logical.min
            + vec2(
                ((point[0] - x) / width) as f32 * self.logical.width(),
                ((point[1] - y) / height) as f32 * self.logical.height(),
            )
    }
}

/// 截屏时屏幕上的一个窗口（由平台层给出）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenWindow {
    /// 全局坐标 `[x, y, 宽, 高]`，单位与 [`Display::frame`] 相同。
    pub frame: [f64; 4],
    /// 是否浮在普通窗口之上（macOS 的窗口层级大于 0、Windows 的置顶窗口）。
    pub floating: bool,
}

/// 浮动窗口覆盖显示器的面积达到这个比例时，当作全屏遮罩跳过。
const FULL_SCREEN_COVERAGE: f32 = 0.95;

/// 一次截屏：显示器的完整图像，以及截屏时屏幕上的窗口。
pub struct Capture {
    pub display: Display,
    pub image: RgbaImage,
    /// 窗口在图像中的位置（像素，已裁到图像范围内），上层的窗口在前。
    pub windows: Vec<Rect>,
}

impl Capture {
    /// `windows` 按从上到下的顺序排列（平台接口返回的就是这个顺序）。
    ///
    /// 与 Snipaste 的 `requestRectDetection(…, excludeFullWins)` 一样跳过（几乎）铺满整个显示器的浮动窗口：
    /// 它们多半是透明的遮罩（例如其他工具的全屏覆盖层），留着会让鼠标下永远只识别出整个屏幕。
    /// 实测有的遮罩比屏幕少一像素（`0, 1, 1920 × 1079`），所以按覆盖面积判断而不是要求完全相等。
    pub fn new(display: Display, image: RgbaImage, windows: &[ScreenWindow]) -> Self {
        let bounds = Rect::from_min_size(
            Pos2::ZERO,
            vec2(image.width() as f32, image.height() as f32),
        );
        let [x, y, width, height] = display.frame;
        let (kx, ky) = (
            f64::from(image.width()) / width,
            f64::from(image.height()) / height,
        );
        let windows = windows
            .iter()
            .filter_map(|window| {
                let [wx, wy, ww, wh] = window.frame;
                let rect = Rect::from_min_max(
                    pos2(
                        ((wx - x) * kx).round() as f32,
                        ((wy - y) * ky).round() as f32,
                    ),
                    pos2(
                        ((wx + ww - x) * kx).round() as f32,
                        ((wy + wh - y) * ky).round() as f32,
                    ),
                )
                .intersect(bounds);
                let covers_all = rect.area() >= bounds.area() * FULL_SCREEN_COVERAGE;
                (rect.width() >= 2.0 && rect.height() >= 2.0 && !(window.floating && covers_all))
                    .then_some(rect)
            })
            .collect();
        Self {
            display,
            image,
            windows,
        }
    }

    /// 整个图像的范围（像素）。
    pub fn bounds(&self) -> Rect {
        Rect::from_min_size(
            Pos2::ZERO,
            vec2(self.image.width() as f32, self.image.height() as f32),
        )
    }

    /// 鼠标下最上层的窗口；桌面上没有窗口时返回 `None`。
    pub fn window_at(&self, point: Pos2) -> Option<Rect> {
        self.windows
            .iter()
            .copied()
            .find(|rect| rect.contains(point))
    }

    /// 某个像素的颜色（RGBA）。
    pub fn pixel(&self, point: Pos2) -> Option<[u8; 4]> {
        let (x, y) = (point.x.floor(), point.y.floor());
        let inside = x >= 0.0
            && y >= 0.0
            && (x as u32) < self.image.width()
            && (y as u32) < self.image.height();
        inside.then(|| self.image.get_pixel(x as u32, y as u32).0)
    }

    /// 裁出选区的图像。
    pub fn crop(&self, rect: Rect) -> RgbaImage {
        let rect = snap(rect).intersect(self.bounds());
        image::imageops::crop_imm(
            &self.image,
            rect.min.x as u32,
            rect.min.y as u32,
            rect.width().max(1.0) as u32,
            rect.height().max(1.0) as u32,
        )
        .to_image()
    }

    /// 逻辑坐标中的区域换算成图像中的像素（裁到图像范围内），[`Self::logical_rect`] 的逆运算。
    pub fn pixel_rect(&self, rect: Rect) -> Rect {
        let logical = self.display.logical;
        let k = self.image.width() as f32 / logical.width();
        Rect::from_min_max(
            ((rect.min - logical.min) * k).to_pos2(),
            ((rect.max - logical.min) * k).to_pos2(),
        )
        .intersect(self.bounds())
    }

    /// 全局坐标中的点（平台单位，例如 `platform::screen::cursor` 的结果）换算成图像中的像素坐标。
    pub fn global_to_pixels(&self, point: [f64; 2]) -> Pos2 {
        let [x, y, width, height] = self.display.frame;
        pos2(
            ((point[0] - x) / width) as f32 * self.image.width() as f32,
            ((point[1] - y) / height) as f32 * self.image.height() as f32,
        )
    }

    /// 选区在 egui 逻辑坐标中的位置（贴图出现在截屏的地方）。
    pub fn logical_rect(&self, rect: Rect) -> Rect {
        let logical = self.display.logical;
        let k = logical.width() / self.image.width() as f32;
        Rect::from_min_max(
            logical.min + rect.min.to_vec2() * k,
            logical.min + rect.max.to_vec2() * k,
        )
    }
}

/// 把矩形的四边取整到整像素（向外取整）。
fn snap(rect: Rect) -> Rect {
    Rect::from_min_max(
        pos2(rect.min.x.floor(), rect.min.y.floor()),
        pos2(rect.max.x.ceil(), rect.max.y.ceil()),
    )
}

/// 两点围成的选区：取整到整像素，限制在 `bounds` 内，至少 1×1。
pub fn rect_between(a: Pos2, b: Pos2, bounds: Rect) -> Rect {
    let rect = snap(Rect::from_two_pos(a, b)).intersect(bounds);
    let max = pos2(
        rect.max.x.max(rect.min.x + 1.0),
        rect.max.y.max(rect.min.y + 1.0),
    );
    Rect::from_min_max(rect.min, max)
}

/// 选区上可以拖动的位置：中间（移动）或八个控制点（调整大小）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Move,
    Left,
    Right,
    Top,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Handle {
    /// 八个控制点（不含中间）。
    pub const RESIZE: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// 控制点在选区上的位置。
    pub fn anchor(self, rect: Rect) -> Pos2 {
        let (x, y) = self.sides();
        let x = match x {
            Some(Side::Min) => rect.min.x,
            Some(Side::Max) => rect.max.x,
            None => rect.center().x,
        };
        let y = match y {
            Some(Side::Min) => rect.min.y,
            Some(Side::Max) => rect.max.y,
            None => rect.center().y,
        };
        pos2(x, y)
    }

    /// 鼠标放在控制点上时的指针形状。
    pub fn cursor(self) -> egui::CursorIcon {
        use egui::CursorIcon;
        match self {
            Handle::Move => CursorIcon::Move,
            Handle::Left | Handle::Right => CursorIcon::ResizeHorizontal,
            Handle::Top | Handle::Bottom => CursorIcon::ResizeVertical,
            Handle::TopLeft | Handle::BottomRight => CursorIcon::ResizeNwSe,
            Handle::TopRight | Handle::BottomLeft => CursorIcon::ResizeNeSw,
        }
    }

    /// 拖动这个控制点时移动的左右边与上下边。
    fn sides(self) -> (Option<Side>, Option<Side>) {
        match self {
            Handle::Move => (None, None),
            Handle::Left => (Some(Side::Min), None),
            Handle::Right => (Some(Side::Max), None),
            Handle::Top => (None, Some(Side::Min)),
            Handle::Bottom => (None, Some(Side::Max)),
            Handle::TopLeft => (Some(Side::Min), Some(Side::Min)),
            Handle::TopRight => (Some(Side::Max), Some(Side::Min)),
            Handle::BottomLeft => (Some(Side::Min), Some(Side::Max)),
            Handle::BottomRight => (Some(Side::Max), Some(Side::Max)),
        }
    }
}

/// 矩形的一条边：左 / 上为 `Min`，右 / 下为 `Max`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Min,
    Max,
}

/// 鼠标在选区的哪个位置：先看控制点（离边 `tolerance` 以内），再看是否在选区内。
/// 选区很小时控制点会重叠，优先四个角。
pub fn hit(rect: Rect, point: Pos2, tolerance: f32) -> Option<Handle> {
    let near = |a: f32, b: f32| (a - b).abs() <= tolerance;
    let inside_x = point.x >= rect.min.x - tolerance && point.x <= rect.max.x + tolerance;
    let inside_y = point.y >= rect.min.y - tolerance && point.y <= rect.max.y + tolerance;
    if !(inside_x && inside_y) {
        return None;
    }
    let (left, right) = (near(point.x, rect.min.x), near(point.x, rect.max.x));
    let (top, bottom) = (near(point.y, rect.min.y), near(point.y, rect.max.y));
    Some(match (left, right, top, bottom) {
        (true, _, true, _) => Handle::TopLeft,
        (_, true, true, _) => Handle::TopRight,
        (true, _, _, true) => Handle::BottomLeft,
        (_, true, _, true) => Handle::BottomRight,
        (true, ..) => Handle::Left,
        (_, true, ..) => Handle::Right,
        (_, _, true, _) => Handle::Top,
        (_, _, _, true) => Handle::Bottom,
        _ if rect.contains(point) => Handle::Move,
        _ => return None,
    })
}

/// 从 `start` 开始拖动控制点 `handle`，鼠标移动了 `delta`（像素）后的选区。
///
/// 移动时整个选区不出图像范围；调整大小时拖过对边会翻转过来，与常见的截图工具一致。
pub fn drag_handle(start: Rect, handle: Handle, delta: Vec2, bounds: Rect) -> Rect {
    let delta = vec2(delta.x.round(), delta.y.round());
    if handle == Handle::Move {
        let dx = delta
            .x
            .clamp(bounds.min.x - start.min.x, bounds.max.x - start.max.x);
        let dy = delta
            .y
            .clamp(bounds.min.y - start.min.y, bounds.max.y - start.max.y);
        return start.translate(vec2(dx, dy));
    }
    let (mut a, mut b) = (start.min, start.max);
    let (x, y) = handle.sides();
    match x {
        Some(Side::Min) => a.x += delta.x,
        Some(Side::Max) => b.x += delta.x,
        None => {}
    }
    match y {
        Some(Side::Min) => a.y += delta.y,
        Some(Side::Max) => b.y += delta.y,
        None => {}
    }
    rect_between(a, b, bounds)
}

/// 方向键：移动选区（`resize` 为 false）或调整右边、下边（宽高各加减一像素）。
pub fn nudge(rect: Rect, delta: Vec2, resize: bool, bounds: Rect) -> Rect {
    if resize {
        rect_between(rect.min, rect.max + delta, bounds)
    } else {
        drag_handle(rect, Handle::Move, delta, bounds)
    }
}

/// 以窗口内的点 `anchor`（相对窗口左上角）为中心，把窗口从 `outer` 缩放到 `size` 后的左上角位置：
/// 缩放前后 `anchor` 下的图像内容保持不动（Snipaste 的“以鼠标位置为中心缩放”）。
pub fn zoom_about(outer: Rect, anchor: Vec2, size: Vec2) -> Pos2 {
    let ratio = size / outer.size().max(Vec2::splat(1.0));
    outer.min + anchor - anchor * ratio
}

/// 把颜色写成 `#RRGGBB`。
pub fn hex_color(rgba: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgba[0], rgba[1], rgba[2])
}

/// 把颜色写成 `rgb(R, G, B)`。
pub fn rgb_color(rgba: [u8; 4]) -> String {
    format!("rgb({}, {}, {})", rgba[0], rgba[1], rgba[2])
}

/// 转成 egui 的纹理数据。
/// 上传纹理用的图片：超过显卡支持的最大边长 `max_side`（`egui::InputState::max_texture_side`）时等比缩小，
/// 否则 egui 直接 panic（例如长截图贴到屏幕上）。裁剪、复制、保存仍然用原图。
///
/// 缩小用 `thumbnail`（整数运算的面积平均，只能缩小）：1404×18178 的长图缩到 8192 高约 150 ms，
/// `resize` 的 Triangle 滤波要 700 多毫秒。
pub fn texture_image(image: &RgbaImage, max_side: usize) -> egui::ColorImage {
    let max = u32::try_from(max_side).unwrap_or(u32::MAX).max(1);
    if image.width() <= max && image.height() <= max {
        return color_image(image);
    }
    let scale = f64::from(max) / f64::from(image.width().max(image.height()));
    let size = |side: u32| ((f64::from(side) * scale).round() as u32).clamp(1, max);
    let resized = image::imageops::thumbnail(image, size(image.width()), size(image.height()));
    color_image(&resized)
}

pub fn color_image(image: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    )
}

/// 图片是否有透明像素（贴图在透明处画白底）。
pub fn has_alpha(image: &RgbaImage) -> bool {
    image.pixels().any(|pixel| pixel.0[3] < 255)
}

/// 旋转 90 度。
pub fn rotate(image: &RgbaImage, clockwise: bool) -> RgbaImage {
    if clockwise {
        image::imageops::rotate90(image)
    } else {
        image::imageops::rotate270(image)
    }
}

/// 水平或垂直翻转。
pub fn flip(image: &RgbaImage, horizontal: bool) -> RgbaImage {
    if horizontal {
        image::imageops::flip_horizontal(image)
    } else {
        image::imageops::flip_vertical(image)
    }
}

/// 编码成 PNG。`fast` 为 true 时用最快的压缩（写剪贴板时用，整屏截图也不会卡顿），
/// 否则用默认压缩（保存文件时用）。
pub fn encode_png(image: &RgbaImage, fast: bool) -> Result<Vec<u8>> {
    let mut png = Vec::new();
    let encoder = if fast {
        PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive)
    } else {
        PngEncoder::new(&mut png)
    };
    image.write_with_encoder(encoder).context("编码 PNG 失败")?;
    Ok(png)
}

/// 只含一张 PNG 图片的剪贴板快照（Windows 上写入时会自动补一份 CF_DIB）。
pub fn image_snapshot(png: Vec<u8>) -> Snapshot {
    Snapshot {
        items: vec![vec![Representation::new(uti::PNG, png)]],
    }
}

/// 取出剪贴板快照中的图片（按 [`uti::IMAGE_TYPES`] 的顺序找第一个能解码的）。
pub fn image_from_snapshot(snapshot: &Snapshot) -> Option<RgbaImage> {
    uti::IMAGE_TYPES.iter().find_map(|kind| {
        let data = snapshot.first(kind)?;
        ImageReader::new(Cursor::new(data))
            .with_guessed_format()
            .ok()?
            .decode()
            .ok()
            .map(|image| image.to_rgba8())
    })
}

/// 保存对话框返回的路径没有 `.png` 后缀时补上（文件名里可能有别的点，不能用 `with_extension` 替换）。
pub fn ensure_png(path: std::path::PathBuf) -> std::path::PathBuf {
    if path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
    {
        return path;
    }
    let mut name = path.into_os_string();
    name.push(".png");
    name.into()
}

/// 保存截图时的默认文件名，例如 `截图 2026-10-19 20.35.12.png`（与系统截屏的命名方式相近）。
pub fn file_name(now: chrono::DateTime<chrono::Local>) -> String {
    format!("截图 {}.png", now.format("%Y-%m-%d %H.%M.%S"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn display() -> Display {
        Display {
            id: 1,
            frame: [100.0, 0.0, 200.0, 100.0],
            logical: Rect::from_min_size(pos2(100.0, 0.0), vec2(200.0, 100.0)),
            scale: 2.0,
        }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(x, y), vec2(w, h))
    }

    #[test]
    fn texture_image_fits_max_texture_side() {
        // 长截图：按比例缩小到最长边不超过上限。
        let long = RgbaImage::new(1210, 11645);
        assert_eq!(texture_image(&long, 8192).size, [851, 8192]);
        // 极细长的图也至少保留 1 像素宽。
        assert_eq!(
            texture_image(&RgbaImage::new(1, 9000), 2048).size,
            [1, 2048]
        );
        // 不超过上限时原样上传。
        assert_eq!(
            texture_image(&RgbaImage::new(300, 200), 2048).size,
            [300, 200]
        );
    }

    #[test]
    fn maps_windows_to_pixels_and_skips_overlays() {
        let image = RgbaImage::new(400, 200);
        let windows = [
            // 全屏的浮动遮罩：跳过。
            ScreenWindow {
                frame: [100.0, 0.0, 200.0, 100.0],
                floating: true,
            },
            // 比屏幕少一像素的浮动遮罩：同样跳过。
            ScreenWindow {
                frame: [100.0, 0.5, 200.0, 99.5],
                floating: true,
            },
            // 普通窗口，部分在显示器左侧之外。
            ScreenWindow {
                frame: [50.0, 10.0, 100.0, 20.0],
                floating: false,
            },
            // 全屏的普通窗口（最大化的应用）：保留。
            ScreenWindow {
                frame: [100.0, 0.0, 200.0, 100.0],
                floating: false,
            },
            // 完全在显示器之外：跳过。
            ScreenWindow {
                frame: [400.0, 0.0, 10.0, 10.0],
                floating: false,
            },
        ];
        let capture = Capture::new(display(), image, &windows);
        assert_eq!(
            capture.windows,
            vec![rect(0.0, 20.0, 100.0, 40.0), rect(0.0, 0.0, 400.0, 200.0)]
        );
        assert_eq!(
            capture.window_at(pos2(10.0, 30.0)),
            Some(rect(0.0, 20.0, 100.0, 40.0))
        );
        assert_eq!(
            capture.window_at(pos2(200.0, 150.0)),
            Some(rect(0.0, 0.0, 400.0, 200.0))
        );
        // 选区换回逻辑坐标：像素除以 2，再加上显示器的位置。
        assert_eq!(
            capture.logical_rect(rect(0.0, 20.0, 100.0, 40.0)),
            rect(100.0, 10.0, 50.0, 20.0)
        );
        assert_eq!(display().to_logical([150.0, 50.0]), pos2(150.0, 50.0));
        assert_eq!(
            capture.pixel_rect(rect(100.0, 10.0, 50.0, 20.0)),
            rect(0.0, 20.0, 100.0, 40.0)
        );
    }

    #[test]
    fn crops_and_reads_pixels() {
        let mut image = RgbaImage::new(4, 4);
        image.put_pixel(2, 1, Rgba([255, 0, 128, 255]));
        let capture = Capture::new(display(), image, &[]);
        assert_eq!(capture.pixel(pos2(2.7, 1.2)), Some([255, 0, 128, 255]));
        assert_eq!(capture.pixel(pos2(4.0, 0.0)), None);
        let cropped = capture.crop(rect(1.5, 0.5, 2.0, 2.0));
        // 向外取整：1.5–3.5 × 0.5–2.5 → 1–4 × 0–3。
        assert_eq!(cropped.dimensions(), (3, 3));
        assert_eq!(cropped.get_pixel(1, 1).0, [255, 0, 128, 255]);
        assert_eq!(hex_color([255, 0, 128, 255]), "#FF0080");
        assert_eq!(rgb_color([255, 0, 128, 255]), "rgb(255, 0, 128)");
    }

    #[test]
    fn builds_selection_between_points() {
        let bounds = rect(0.0, 0.0, 100.0, 50.0);
        assert_eq!(
            rect_between(pos2(30.4, 40.0), pos2(10.6, 5.2), bounds),
            Rect::from_min_max(pos2(10.0, 5.0), pos2(31.0, 40.0))
        );
        // 拖出图像之外时限制在图像内。
        assert_eq!(
            rect_between(pos2(-5.0, -5.0), pos2(500.0, 20.0), bounds),
            rect(0.0, 0.0, 100.0, 20.0)
        );
        // 至少 1×1。
        assert_eq!(
            rect_between(pos2(7.0, 7.0), pos2(7.0, 7.0), bounds),
            rect(7.0, 7.0, 1.0, 1.0)
        );
    }

    #[test]
    fn hits_handles() {
        let sel = rect(10.0, 10.0, 100.0, 50.0);
        assert_eq!(hit(sel, pos2(11.0, 9.0), 4.0), Some(Handle::TopLeft));
        assert_eq!(hit(sel, pos2(60.0, 61.0), 4.0), Some(Handle::Bottom));
        assert_eq!(hit(sel, pos2(108.0, 35.0), 4.0), Some(Handle::Right));
        assert_eq!(hit(sel, pos2(60.0, 35.0), 4.0), Some(Handle::Move));
        assert_eq!(hit(sel, pos2(200.0, 35.0), 4.0), None);
        assert_eq!(Handle::BottomRight.anchor(sel), pos2(110.0, 60.0));
        assert_eq!(Handle::Top.anchor(sel), pos2(60.0, 10.0));
    }

    #[test]
    fn drags_handles() {
        let bounds = rect(0.0, 0.0, 200.0, 100.0);
        let sel = rect(10.0, 10.0, 50.0, 50.0);
        // 移动：不出图像范围。
        assert_eq!(
            drag_handle(sel, Handle::Move, vec2(500.0, -3.4), bounds),
            rect(150.0, 7.0, 50.0, 50.0)
        );
        // 拖右下角放大。
        assert_eq!(
            drag_handle(sel, Handle::BottomRight, vec2(10.0, 5.0), bounds),
            rect(10.0, 10.0, 60.0, 55.0)
        );
        // 左边拖过右边：翻转。
        assert_eq!(
            drag_handle(sel, Handle::Left, vec2(70.0, 0.0), bounds),
            Rect::from_min_max(pos2(60.0, 10.0), pos2(80.0, 60.0))
        );
        // 方向键。
        assert_eq!(
            nudge(sel, vec2(1.0, 0.0), false, bounds),
            rect(11.0, 10.0, 50.0, 50.0)
        );
        assert_eq!(
            nudge(sel, vec2(0.0, -1.0), true, bounds),
            rect(10.0, 10.0, 50.0, 49.0)
        );
    }

    #[test]
    fn zooms_about_anchor() {
        let outer = rect(100.0, 100.0, 200.0, 100.0);
        // 以中心放大一倍：中心不动。
        assert_eq!(
            zoom_about(outer, vec2(100.0, 50.0), vec2(400.0, 200.0)),
            pos2(0.0, 50.0)
        );
        // 以左上角缩放：位置不变。
        assert_eq!(
            zoom_about(outer, Vec2::ZERO, vec2(100.0, 50.0)),
            pos2(100.0, 100.0)
        );
    }

    #[test]
    fn transforms_and_round_trips_images() {
        let mut image = RgbaImage::new(3, 2);
        image.put_pixel(0, 0, Rgba([1, 2, 3, 255]));
        let rotated = rotate(&image, true);
        assert_eq!(rotated.dimensions(), (2, 3));
        assert_eq!(rotated.get_pixel(1, 0).0, [1, 2, 3, 255]);
        assert_eq!(rotate(&rotated, false), image);
        assert_eq!(flip(&image, true).get_pixel(2, 0).0, [1, 2, 3, 255]);
        assert_eq!(flip(&image, false).get_pixel(0, 1).0, [1, 2, 3, 255]);
        assert!(has_alpha(&image));

        for fast in [true, false] {
            let png = encode_png(&image, fast).unwrap();
            let snapshot = image_snapshot(png);
            assert_eq!(image_from_snapshot(&snapshot), Some(image.clone()));
        }
        assert_eq!(image_from_snapshot(&Snapshot::from_text("x")), None);
    }

    #[test]
    fn names_files() {
        use chrono::TimeZone;
        let time = chrono::Local
            .with_ymd_and_hms(2026, 10, 19, 20, 35, 12)
            .unwrap();
        assert_eq!(file_name(time), "截图 2026-10-19 20.35.12.png");
        assert_eq!(
            ensure_png("a/截图 20.35.12".into()),
            std::path::PathBuf::from("a/截图 20.35.12.png")
        );
        assert_eq!(
            ensure_png("a/b.PNG".into()),
            std::path::PathBuf::from("a/b.PNG")
        );
    }
}
