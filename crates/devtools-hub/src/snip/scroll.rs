//! 滚动截图（长截图）的拼接，分析见 `docs/微信滚动截图与文字提取实现分析.md`。
//!
//! 微信用 OpenCV 的 ORB 特征点加汉明距离匹配算两帧之间的位移（`GetMergeOffsetInner`）。滚动只是整像素的平移，
//! 不需要应对缩放旋转，这里用“行指纹”：
//!
//! 1. 每行取中间的一段（两侧各留一点边距，避开滚动时才出现的滚动条）算哈希，并记下这行是否有内容（不是纯色）；
//! 2. 两帧顶部、底部位置不变的行是固定的标题栏、输入框等，只在中间的滚动区域里找位移；
//! 3. 新一帧每个有内容的行，到上一帧里找哈希相同的行，按两者的行号差投票，得票最多的就是滚动的距离
//!    （鼠标悬停的高亮、个别变化的行只少几票，不影响结果）；重复太多的哈希（分隔线等）不参与投票；
//! 4. 把新一帧滚动区域底部新露出来的行接到长图下面。
//!
//! 两种情况投票找不到位移：
//! - 画面在原地变了（鼠标悬停的高亮、动图、闪烁的光标）：多数行还在原来的位置，当作没有滚动，换成新的一帧作比较；
//! - 滚动的距离不是整像素（触控板的平滑滚动），文字被重新采样，哈希几乎都对不上：改用每行分块的平均亮度
//!   近似比较，找差别最小的位移。
//!
//! 与上一帧对不上时（往回滚、页面跳动、上一帧有临时的变化），再到整张长图里找这一帧：落在已经拼过的位置时
//! 提示“往回滚了”，等向下滚动超过长图底部再继续；露出了长图底部以下的内容就从那里接上。
//! 长图里也找不到（滚得太快，与已拼的内容没有重合）时跳过这一帧。

use std::collections::HashMap;

use image::RgbaImage;

/// 两侧不参与指纹的边距（像素）最多这么宽：macOS 的浮动滚动条连同留白约 16 点，Retina 屏上 32 像素。
const MAX_MARGIN: u32 = 40;
/// 一行亮度的最大值与最小值相差超过这个数才算有内容（纯色、很浅的渐变不算）。
const CONTENT_CONTRAST: u8 = 16;
/// 一个哈希在上一帧出现超过这么多次时不参与投票（分隔线、重复的花纹，行号差没有意义）。
const MAX_REPEAT: usize = 8;
/// 位移至少要有这么多票。
const MIN_VOTES: usize = 4;
/// 重合部分里有内容的行，至少这个比例要投给同一个位移。
const MIN_AGREEMENT: f32 = 0.5;
/// 长图最多这么多像素（RGBA 每像素 4 字节，约 200 MB）。
const MAX_PIXELS: u64 = 50_000_000;
/// 长图最高这么多行（常见显卡、图片查看器支持的上限）。
const MAX_HEIGHT: u32 = 32_000;
/// 每行分成这么多块记录平均亮度，用于近似比较。
const BLOCKS: usize = 16;
/// 原地变化：上一帧里有内容、不重复的行，至少这个比例还在原来的位置。
const IN_PLACE_RATIO: f32 = 0.6;
/// 近似比较时，每块平均亮度的平均差别（0–255）不超过这个数才算对上。
const MAX_APPROX_DIFF: f32 = 4.0;
/// 近似比较时，重合部分至少要有这么多有内容的行。
const MIN_APPROX_ROWS: usize = 8;
/// 预览缩略图的宽度（像素）：截屏遮罩上的预览约 140 点宽，Retina 屏上 280 像素。
pub const THUMBNAIL_WIDTH: u32 = 280;

/// 一行的指纹。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Row {
    hash: u64,
    /// 这行有内容（不是纯色），可以用来投票。
    content: bool,
    /// 分块的平均亮度，用于近似比较。
    blocks: [u8; BLOCKS],
}

/// 新一帧与上一帧相比的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shift {
    /// 画面没有变化。
    Same,
    /// 内容向上滚动了 `dy` 行；底部有 `bottom` 行固定不动。
    Scrolled { dy: usize, bottom: usize },
    /// 没有滚动，但画面在原地变了（悬停高亮、动图等）。
    InPlace,
    /// 找不到可靠的位移：往回滚、滚得太快，或者画面整个变了。
    Lost,
}

/// 拼接一帧的结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// 画面没有变化。
    Unchanged,
    /// 接上了 `rows` 行新内容。
    Appended { rows: usize },
    /// 这一帧落在已经拼过的位置（往回滚了），没有新内容。
    Behind,
    /// 这一帧接不上（滚得太快、画面整个变了），已跳过。
    Lost,
    /// 长图达到了最大高度，不再增长。
    Full,
}

/// 滚动截图的拼接器：依次送入同一块区域的截图，得到拼好的长图。
pub struct Stitcher {
    width: u32,
    /// 每一帧的高度。
    frame_height: usize,
    /// 已经拼好的部分（RGBA，逐行存放），到上一帧滚动区域的底边为止，不含上一帧底部固定的行。
    canvas: Vec<u8>,
    /// `canvas` 每一行的指纹（与上一帧对不上时到整张长图里找）。
    canvas_rows: Vec<Row>,
    /// 上一帧（最后一次接上的帧）与它的指纹。
    last: RgbaImage,
    last_rows: Vec<Row>,
    /// 上一帧底部固定不动的行数（输入框、底栏），最后才接到长图末尾。
    footer: usize,
    /// 长图最高多少行。
    max_height: usize,
    full: bool,
    /// `canvas` 的缩略图，随拼接逐步更新（每次都缩放整张长图太慢）。
    thumbnail: Thumbnail,
}

impl Stitcher {
    /// 从第一帧开始。
    pub fn new(first: RgbaImage) -> Self {
        let (width, height) = (first.width(), first.height() as usize);
        let max_height = (MAX_PIXELS / u64::from(width.max(1))).min(u64::from(MAX_HEIGHT)) as usize;
        let mut stitcher = Self {
            width,
            frame_height: height,
            canvas: first.as_raw().clone(),
            canvas_rows: fingerprint(&first),
            last_rows: fingerprint(&first),
            last: first,
            footer: 0,
            max_height: max_height.max(height),
            full: false,
            thumbnail: Thumbnail::new(width),
        };
        stitcher.thumbnail.sync(&stitcher.canvas);
        stitcher
    }

    /// 送入新的一帧（大小必须与第一帧相同，不同时当作接不上）。
    pub fn push(&mut self, frame: RgbaImage) -> Step {
        if self.full {
            return Step::Full;
        }
        if frame.dimensions() != self.last.dimensions() {
            return Step::Lost;
        }
        let rows = fingerprint(&frame);
        let (dy, bottom) = match find_shift(&self.last_rows, &rows) {
            Shift::Same => return Step::Unchanged,
            Shift::Lost => return self.relocate(frame, rows),
            Shift::InPlace => {
                // 位置没变，之后拿新的画面比较（长图底部固定的行也取自它）。
                self.last = frame;
                self.last_rows = rows;
                return Step::Unchanged;
            }
            Shift::Scrolled { dy, bottom } => (dy, bottom),
        };
        let stride = self.width as usize * 4;
        let height = self.frame_height;
        // 底部固定的行数变了：多出来的固定行原先被当作内容接上了，去掉；少了的从上一帧补上。
        if bottom > self.footer {
            let remove = (bottom - self.footer).min(self.canvas_rows.len());
            self.canvas.truncate(self.canvas.len() - remove * stride);
            self.canvas_rows.truncate(self.canvas_rows.len() - remove);
        } else if bottom < self.footer {
            let raw = self.last.as_raw();
            self.canvas.extend_from_slice(
                &raw[(height - self.footer) * stride..(height - bottom) * stride],
            );
            self.canvas_rows
                .extend_from_slice(&self.last_rows[height - self.footer..height - bottom]);
        }
        self.footer = bottom;
        // 新露出来的行：新一帧滚动区域底部的 `dy` 行。
        self.append(frame, rows, height - bottom - dy)
    }

    /// 与上一帧对不上：到整张长图里找这一帧。落在已经拼过的位置时什么也不做；
    /// 露出了长图底部以下的内容时从那里接上（这一帧成为新的上一帧）。
    fn relocate(&mut self, frame: RgbaImage, rows: Vec<Row>) -> Step {
        let Some(offset) = locate(&self.canvas_rows, &rows) else {
            return Step::Lost;
        };
        // 这一帧第 r 行是长图第 offset + r 行；长图之后的行从这一帧的第 `first_new` 行开始。
        let first_new = self.canvas_rows.len() as isize - offset;
        let content_end = (self.frame_height - self.footer) as isize;
        if first_new >= content_end {
            return Step::Behind;
        }
        self.append(frame, rows, first_new.max(0) as usize)
    }

    /// 把这一帧滚动区域里从第 `start` 行开始的新内容接到长图下面（到高度上限为止），这一帧成为新的上一帧。
    fn append(&mut self, frame: RgbaImage, rows: Vec<Row>, start: usize) -> Step {
        let stride = self.width as usize * 4;
        let end = self.frame_height - self.footer;
        let wanted = end.saturating_sub(start);
        let room = self
            .max_height
            .saturating_sub(self.canvas_rows.len() + self.footer);
        let added = wanted.min(room);
        self.canvas
            .extend_from_slice(&frame.as_raw()[start * stride..(start + added) * stride]);
        self.canvas_rows
            .extend_from_slice(&rows[start..start + added]);
        self.last = frame;
        self.last_rows = rows;
        self.thumbnail.sync(&self.canvas);
        if added < wanted {
            self.full = true;
            return Step::Full;
        }
        Step::Appended { rows: added }
    }

    /// 长图的宽度（像素，与每一帧相同）。
    pub fn width(&self) -> u32 {
        self.width
    }

    /// 长图现在的高度（像素）。
    pub fn height(&self) -> u32 {
        (self.canvas.len() / (self.width as usize * 4) + self.footer) as u32
    }

    /// 拼好的长图：已拼好的部分加上最后一帧底部固定的行。
    pub fn image(&self) -> RgbaImage {
        let stride = self.width as usize * 4;
        let raw = self.last.as_raw();
        let mut pixels = Vec::with_capacity(self.canvas.len() + self.footer * stride);
        pixels.extend_from_slice(&self.canvas);
        pixels.extend_from_slice(&raw[(self.frame_height - self.footer) * stride..]);
        RgbaImage::from_raw(self.width, self.height(), pixels).expect("长图大小与像素数一致")
    }

    /// 预览图：长图缩小到 [`THUMBNAIL_WIDTH`] 像素宽（本来更窄时不缩小），太高时只保留最下面 `max_height` 像素
    /// （最新拼上的部分）。
    pub fn preview(&self, max_height: u32) -> RgbaImage {
        let thumbnail = &self.thumbnail;
        let stride = thumbnail.width * 4;
        // 底部固定的行不在 `canvas` 里，单独缩小后接在后面。
        let footer = {
            let raw = self.last.as_raw();
            let rows = &raw[(self.frame_height - self.footer) * self.width as usize * 4..];
            let mut footer = Thumbnail::new(self.width);
            footer.sync(rows);
            footer.rows
        };
        let total = thumbnail.rows.len() / stride + footer.len() / stride;
        let keep = total.min(max_height as usize).max(1);
        let mut pixels = Vec::with_capacity(keep * stride);
        let skip = (total - keep.min(total)) * stride;
        let body = thumbnail.rows.get(skip..).unwrap_or_default();
        pixels.extend_from_slice(body);
        pixels.extend_from_slice(&footer[skip.saturating_sub(thumbnail.rows.len())..]);
        let rows = pixels.len() / stride;
        if rows == 0 {
            return RgbaImage::new(thumbnail.width as u32, 1);
        }
        RgbaImage::from_raw(thumbnail.width as u32, rows as u32, pixels)
            .expect("缩略图大小与像素数一致")
    }
}

/// 逐步更新的缩略图：长图每增加一些行，只缩小新增的部分（按块取平均）。
struct Thumbnail {
    /// 原图宽度（像素）。
    source_width: usize,
    /// 缩略图宽度（像素）。
    width: usize,
    /// 每个缩略图像素对应原图的多少像素（不小于 1）。
    step: f64,
    /// 缩略图每一列对应原图的列范围。
    columns: Vec<(usize, usize)>,
    /// 已经缩好的行（RGBA）。
    rows: Vec<u8>,
}

impl Thumbnail {
    fn new(source_width: u32) -> Self {
        let source_width = source_width.max(1) as usize;
        let width = source_width.min(THUMBNAIL_WIDTH as usize);
        let step = source_width as f64 / width as f64;
        let columns = (0..width)
            .map(|i| {
                let start = (i as f64 * step) as usize;
                let end = (((i + 1) as f64 * step) as usize).clamp(start + 1, source_width);
                (start, end)
            })
            .collect();
        Self {
            source_width,
            width,
            step,
            columns,
            rows: Vec::new(),
        }
    }

    /// 缩略图第 `row` 行对应原图的行范围。
    fn source_rows(&self, row: usize) -> (usize, usize) {
        let start = (row as f64 * self.step) as usize;
        let end = (((row + 1) as f64 * self.step) as usize).max(start + 1);
        (start, end)
    }

    /// 跟上原图 `source`（RGBA，逐行存放）：原图变短时去掉超出的行，变长时缩小新增的部分。
    fn sync(&mut self, source: &[u8]) {
        let source_stride = self.source_width * 4;
        let available = source.len() / source_stride;
        let stride = self.width * 4;
        // 去掉对应原图行已经不存在的缩略图行。
        while self.rows.len() >= stride
            && self.source_rows(self.rows.len() / stride - 1).1 > available
        {
            self.rows.truncate(self.rows.len() - stride);
        }
        loop {
            let (start, end) = self.source_rows(self.rows.len() / stride);
            if end > available {
                break;
            }
            for &(x0, x1) in &self.columns {
                let mut sum = [0u32; 4];
                for y in start..end {
                    let line = &source[y * source_stride..(y + 1) * source_stride];
                    for pixel in line[x0 * 4..x1 * 4].as_chunks::<4>().0 {
                        for (total, &value) in sum.iter_mut().zip(pixel) {
                            *total += u32::from(value);
                        }
                    }
                }
                let count = ((end - start) * (x1 - x0)) as u32;
                self.rows.extend(sum.map(|total| (total / count) as u8));
            }
        }
    }
}

/// 每一行的指纹：中间一段（两侧留出边距）像素的哈希、这行是否有内容，以及分块的平均亮度。
fn fingerprint(image: &RgbaImage) -> Vec<Row> {
    let width = image.width();
    let margin = (width / 8).min(MAX_MARGIN);
    let (x0, x1) = (margin as usize, (width - margin) as usize);
    let stride = width as usize * 4;
    let span = (x1 - x0).max(1);
    image
        .as_raw()
        .chunks_exact(stride)
        .map(|row| {
            // FNV-1a。
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            let (mut low, mut high) = (u8::MAX, u8::MIN);
            let mut sums = [0u32; BLOCKS];
            for (i, pixel) in row[x0 * 4..x1 * 4].as_chunks::<4>().0.iter().enumerate() {
                for &byte in &pixel[..3] {
                    hash ^= u64::from(byte);
                    hash = hash.wrapping_mul(0x0100_0000_01b3);
                }
                let luma = ((u32::from(pixel[0]) * 299
                    + u32::from(pixel[1]) * 587
                    + u32::from(pixel[2]) * 114)
                    / 1000) as u8;
                low = low.min(luma);
                high = high.max(luma);
                sums[i * BLOCKS / span] += u32::from(luma);
            }
            let mut blocks = [0u8; BLOCKS];
            for (index, (block, sum)) in blocks.iter_mut().zip(sums).enumerate() {
                let count = (index + 1) * span / BLOCKS - index * span / BLOCKS;
                *block = (sum / count.max(1) as u32) as u8;
            }
            Row {
                hash,
                content: high.saturating_sub(low) > CONTENT_CONTRAST,
                blocks,
            }
        })
        .collect()
}

/// 比较上一帧 `prev` 与新一帧 `next` 的指纹，找出内容向上滚动的距离。
fn find_shift(prev: &[Row], next: &[Row]) -> Shift {
    let height = prev.len();
    if next.len() != height {
        return Shift::Lost;
    }
    // 顶部、底部位置不变的行：固定的标题栏、输入框（一样的空白行也算进来，不影响结果）。
    let top = prev
        .iter()
        .zip(next)
        .take_while(|(a, b)| a.hash == b.hash)
        .count();
    if top == height {
        return Shift::Same;
    }
    let bottom = prev
        .iter()
        .rev()
        .zip(next.iter().rev())
        .take_while(|(a, b)| a.hash == b.hash)
        .count();
    let end = height - bottom;

    // 上一帧滚动区域里每个哈希出现的行号。
    let mut lines: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, row) in prev.iter().enumerate().take(end).skip(top) {
        if row.content {
            lines.entry(row.hash).or_default().push(index);
        }
    }
    // 新一帧第 r 行与上一帧第 p 行相同，说明内容向上滚动了 p - r 行。
    let mut votes = vec![0usize; end - top];
    for (r, row) in next.iter().enumerate().take(end).skip(top) {
        if !row.content {
            continue;
        }
        let Some(positions) = lines.get(&row.hash).filter(|p| p.len() <= MAX_REPEAT) else {
            continue;
        };
        for &p in positions.iter().filter(|&&p| p > r) {
            votes[p - r] += 1;
        }
    }
    // 得票最多的位移；票数相同取较小的（定时截取，两帧之间滚得不会太远）。
    let Some((dy, &count)) = votes
        .iter()
        .enumerate()
        .skip(1)
        .max_by(|(a_dy, a), (b_dy, b)| a.cmp(b).then(b_dy.cmp(a_dy)))
    else {
        return Shift::Lost;
    };
    // 重合部分（新一帧的 top..end-dy 行）里有内容的行，要有足够多投给了这个位移。
    let overlap = next[top..end - dy].iter().filter(|row| row.content).count();
    if count >= MIN_VOTES && count as f32 >= overlap as f32 * MIN_AGREEMENT {
        return Shift::Scrolled { dy, bottom };
    }
    if changed_in_place(prev, next) {
        return Shift::InPlace;
    }
    match approximate_shift(prev, next, top, end) {
        Some(dy) => Shift::Scrolled { dy, bottom },
        None => Shift::Lost,
    }
}

/// 在整张长图（每行的指纹 `canvas`）里找新一帧 `next`：返回这一帧第 0 行对应长图的第几行
/// （可能是负数：这一帧的上部在长图开头之前）。与 [`find_shift`] 一样按行号差投票。
fn locate(canvas: &[Row], next: &[Row]) -> Option<isize> {
    let mut lines: HashMap<u64, Vec<usize>> = HashMap::new();
    for (index, row) in canvas.iter().enumerate() {
        if row.content {
            lines.entry(row.hash).or_default().push(index);
        }
    }
    let mut votes: HashMap<isize, usize> = HashMap::new();
    for (r, row) in next.iter().enumerate() {
        if !row.content {
            continue;
        }
        let Some(positions) = lines.get(&row.hash).filter(|p| p.len() <= MAX_REPEAT) else {
            continue;
        };
        for &p in positions {
            *votes.entry(p as isize - r as isize).or_default() += 1;
        }
    }
    let (offset, count) = votes
        .into_iter()
        .max_by(|(a_offset, a), (b_offset, b)| a.cmp(b).then(b_offset.cmp(a_offset)))?;
    // 这一帧落在长图范围内的部分里有内容的行，要有足够多投给了这个位置。
    let inside = next
        .iter()
        .enumerate()
        .filter(|(r, row)| {
            let index = offset + *r as isize;
            row.content && index >= 0 && (index as usize) < canvas.len()
        })
        .count();
    (count >= MIN_VOTES && count as f32 >= inside as f32 * MIN_AGREEMENT).then_some(offset)
}

/// 画面是否只在原地变了：上一帧整帧里有内容、不重复的行，多数还在原来的位置。
fn changed_in_place(prev: &[Row], next: &[Row]) -> bool {
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for row in prev.iter().filter(|row| row.content) {
        *counts.entry(row.hash).or_default() += 1;
    }
    let (mut total, mut kept) = (0usize, 0usize);
    for (a, b) in prev.iter().zip(next) {
        if a.content && counts[&a.hash] <= MAX_REPEAT {
            total += 1;
            kept += usize::from(a.hash == b.hash);
        }
    }
    total >= MIN_APPROX_ROWS && kept as f32 >= total as f32 * IN_PLACE_RATIO
}

/// 近似比较滚动区域（`top..end` 行）：找使新一帧与上一帧对应行分块平均亮度差别最小的位移。
/// 差别要足够小，并且明显小于不滚动时的差别（否则画面是整个变了）。
fn approximate_shift(prev: &[Row], next: &[Row], top: usize, end: usize) -> Option<usize> {
    let len = end - top;
    // 重合太少时偶然对上的可能性大。
    let min_overlap = (len / 4).max(16);
    let diff = |dy: usize| -> Option<f32> {
        let (mut sum, mut rows) = (0u32, 0usize);
        for r in top..end - dy {
            let (a, b) = (&next[r], &prev[r + dy]);
            if !a.content {
                continue;
            }
            sum += a
                .blocks
                .iter()
                .zip(&b.blocks)
                .map(|(x, y)| u32::from(x.abs_diff(*y)))
                .sum::<u32>();
            rows += 1;
        }
        (rows >= MIN_APPROX_ROWS).then(|| sum as f32 / (rows * BLOCKS) as f32)
    };
    let still = diff(0)?;
    let (dy, best) = (1..len.saturating_sub(min_overlap))
        .filter_map(|dy| diff(dy).map(|d| (dy, d)))
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    (best <= MAX_APPROX_DIFF && best <= still * 0.5).then_some(dy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// 一张“网页”：每行有不同长度的深色“文字”，行与行之间有空白，像真实的文字排版。
    fn page(width: u32, height: u32) -> RgbaImage {
        RgbaImage::from_fn(width, height, |x, y| {
            let line = y % 24;
            let length = 40 + (y / 24 * 37 % 200);
            if (4..16).contains(&line) && x > 10 && x < 10 + length && (x + y / 24) % 7 != 0 {
                Rgba([(30 + y % 50) as u8, 40, (60 + x % 40) as u8, 255])
            } else {
                Rgba([250, 250, 250, 255])
            }
        })
    }

    /// 从 `page` 里取出可视区域：顶部 `header` 行和底部 `footer` 行固定，中间显示从 `scroll` 开始的内容。
    fn view(page: &RgbaImage, height: u32, scroll: u32, header: u32, footer: u32) -> RgbaImage {
        let width = page.width();
        RgbaImage::from_fn(width, height, |x, y| {
            if y < header {
                Rgba([200, (y * 9 % 255) as u8, (x % 255) as u8, 255])
            } else if y >= height - footer {
                Rgba([(x * 3 % 255) as u8, 90, (y * 5 % 255) as u8, 255])
            } else {
                *page.get_pixel(x, y - header + scroll)
            }
        })
    }

    #[test]
    fn stitches_scrolled_frames_into_the_full_page() {
        let content = page(300, 1200);
        let frames = [0, 37, 90, 91, 200, 333, 480];
        let mut stitcher = Stitcher::new(view(&content, 300, frames[0], 0, 0));
        for pair in frames.windows(2) {
            let step = stitcher.push(view(&content, 300, pair[1], 0, 0));
            assert_eq!(
                step,
                Step::Appended {
                    rows: (pair[1] - pair[0]) as usize
                }
            );
        }
        let image = stitcher.image();
        assert_eq!(image.height(), 300 + 480);
        let expected = image::imageops::crop_imm(&content, 0, 0, 300, 780).to_image();
        assert!(image == expected, "拼出的长图应与原网页的前 780 行一致");
    }

    #[test]
    fn fixed_header_and_footer_appear_once() {
        let content = page(320, 2000);
        let (height, header, footer) = (400, 40, 50);
        let mut stitcher = Stitcher::new(view(&content, height, 0, header, footer));
        let mut last = 0;
        for scroll in [60, 130, 131, 250, 400, 560] {
            let step = stitcher.push(view(&content, height, scroll, header, footer));
            assert_eq!(
                step,
                Step::Appended {
                    rows: (scroll - last) as usize
                }
            );
            last = scroll;
        }
        let image = stitcher.image();
        let visible = height - header - footer;
        assert_eq!(image.height(), header + visible + last + footer);
        let first = view(&content, height, 0, header, footer);
        let final_view = view(&content, height, last, header, footer);
        for x in [0, 100, 319] {
            // 标题栏只在最上面出现一次，下面紧接着网页的开头。
            for y in 0..header {
                assert_eq!(image.get_pixel(x, y), first.get_pixel(x, y));
            }
            for y in 0..visible + last {
                assert_eq!(image.get_pixel(x, header + y), content.get_pixel(x, y));
            }
            // 底栏只在最下面出现一次。
            for y in 0..footer {
                assert_eq!(
                    image.get_pixel(x, image.height() - footer + y),
                    final_view.get_pixel(x, height - footer + y)
                );
            }
        }
    }

    #[test]
    fn unchanged_backwards_and_too_fast_frames_are_skipped() {
        let content = page(240, 3000);
        let mut stitcher = Stitcher::new(view(&content, 300, 100, 0, 0));
        assert_eq!(
            stitcher.push(view(&content, 300, 100, 0, 0)),
            Step::Unchanged
        );
        // 往回滚：落在已经拼过的位置。
        assert_eq!(stitcher.push(view(&content, 300, 60, 0, 0)), Step::Behind);
        // 滚得太快：两帧没有重合。
        assert_eq!(stitcher.push(view(&content, 300, 900, 0, 0)), Step::Lost);
        assert_eq!(stitcher.height(), 300);
        // 滚回有重合的位置后继续。
        assert_eq!(
            stitcher.push(view(&content, 300, 180, 0, 0)),
            Step::Appended { rows: 80 }
        );
        // 大小不同的帧接不上。
        assert_eq!(stitcher.push(RgbaImage::new(240, 200)), Step::Lost);
    }

    #[test]
    fn scrollbar_and_hover_changes_do_not_break_matching() {
        let content = page(400, 1500);
        let mut stitcher = Stitcher::new(view(&content, 360, 0, 0, 0));
        let mut frame = view(&content, 360, 120, 0, 0);
        // 滚动时右边出现滚动条（每一行最右边都变了）。
        for y in 0..360 {
            for x in 388..400 {
                frame.put_pixel(x, y, Rgba([120, 120, 120, 255]));
            }
        }
        // 鼠标悬停的一行高亮。
        for y in 200..224 {
            for x in 0..400 {
                frame.put_pixel(x, y, Rgba([210, 230, 255, 255]));
            }
        }
        assert_eq!(stitcher.push(frame), Step::Appended { rows: 120 });
    }

    /// 内容滚动 `scroll` 行（可以是小数），小数部分在相邻两行之间线性插值，模拟平滑滚动时文字被重新采样。
    fn fractional_view(page: &RgbaImage, height: u32, scroll: f32) -> RgbaImage {
        let base = scroll.floor() as u32;
        let t = scroll - scroll.floor();
        RgbaImage::from_fn(page.width(), height, |x, y| {
            let a = page.get_pixel(x, y + base).0;
            let b = page.get_pixel(x, y + base + 1).0;
            Rgba(std::array::from_fn(|i| {
                (f32::from(a[i]) * (1.0 - t) + f32::from(b[i]) * t).round() as u8
            }))
        })
    }

    #[test]
    fn fractional_scrolling_matches_approximately() {
        let content = page(400, 2000);
        let mut stitcher = Stitcher::new(view(&content, 300, 0, 0, 0));
        // 第一帧是整数位置，之后都落在半个像素上：哈希几乎都对不上，靠近似比较。
        // 近似比较可能差一行。
        let mut last = 0.0;
        for scroll in [40.5, 95.5, 150.3] {
            let Step::Appended { rows } = stitcher.push(fractional_view(&content, 300, scroll))
            else {
                panic!("滚动到 {scroll} 时应该接上");
            };
            assert!(
                (rows as f32 - (scroll - last)).abs() <= 1.0,
                "滚动到 {scroll}：接上了 {rows} 行"
            );
            last = scroll;
        }
        assert!((stitcher.height() as f32 - 450.3).abs() <= 2.0);
    }

    #[test]
    fn in_place_changes_are_not_lost() {
        let content = page(400, 1500);
        let mut stitcher = Stitcher::new(view(&content, 360, 0, 0, 0));
        // 鼠标悬停让一行高亮：没有滚动，也不算跟丢。
        let mut hovered = view(&content, 360, 0, 0, 0);
        for y in 100..124 {
            for x in 0..400 {
                hovered.put_pixel(x, y, Rgba([210, 230, 255, 255]));
            }
        }
        assert_eq!(stitcher.push(hovered.clone()), Step::Unchanged);
        assert!(stitcher.last == hovered, "之后拿悬停后的画面比较");
        assert_eq!(stitcher.height(), 360);
        // 接着滚动照常拼接。
        assert_eq!(
            stitcher.push(view(&content, 360, 90, 0, 0)),
            Step::Appended { rows: 90 }
        );
    }

    #[test]
    fn recovers_after_scrolling_back_or_jumping() {
        let content = page(400, 4000);
        let mut stitcher = Stitcher::new(view(&content, 300, 0, 0, 0));
        for scroll in [100, 200, 300, 400] {
            stitcher.push(view(&content, 300, scroll, 0, 0));
        }
        assert_eq!(stitcher.height(), 700);
        // 跳回开头（例如页面自己滚到顶部），再一路往下滚：在已经拼过的部分里都是“往回滚了”。
        assert_eq!(stitcher.push(view(&content, 300, 0, 0, 0)), Step::Behind);
        assert_eq!(stitcher.push(view(&content, 300, 250, 0, 0)), Step::Behind);
        // 滚过上一帧的位置后照常拼接，长图与原网页一致。
        assert_eq!(
            stitcher.push(view(&content, 300, 520, 0, 0)),
            Step::Appended { rows: 120 }
        );
        let image = stitcher.image();
        let expected = image::imageops::crop_imm(&content, 0, 0, 400, 820).to_image();
        assert!(image == expected);
    }

    #[test]
    fn blank_frames_never_match() {
        let blank = RgbaImage::from_pixel(200, 200, Rgba([255, 255, 255, 255]));
        let mut stitcher = Stitcher::new(blank.clone());
        let mut other = blank;
        other.put_pixel(100, 100, Rgba([0, 0, 0, 255]));
        assert_eq!(stitcher.push(other), Step::Lost);
    }

    #[test]
    fn stops_growing_at_max_height() {
        let content = page(200, 6000);
        let mut stitcher = Stitcher::new(view(&content, 300, 0, 0, 0));
        stitcher.max_height = 500;
        assert_eq!(
            stitcher.push(view(&content, 300, 150, 0, 0)),
            Step::Appended { rows: 150 }
        );
        assert_eq!(stitcher.push(view(&content, 300, 300, 0, 0)), Step::Full);
        assert!(stitcher.full);
        assert_eq!(stitcher.height(), 500);
        assert_eq!(stitcher.push(view(&content, 300, 310, 0, 0)), Step::Full);
    }

    #[test]
    fn preview_keeps_the_newest_part() {
        let content = page(400, 3000);
        let mut stitcher = Stitcher::new(view(&content, 300, 0, 0, 0));
        for scroll in (100..=1500).step_by(100) {
            stitcher.push(view(&content, 300, scroll, 0, 0));
        }
        assert_eq!(stitcher.height(), 1800);
        // 缩小到 280 像素宽（0.7 倍），只保留最下面 200 行。
        let preview = stitcher.preview(200);
        assert_eq!((preview.width(), preview.height()), (280, 200));
        let full = image::imageops::resize(
            &stitcher.image(),
            280,
            1260,
            image::imageops::FilterType::Triangle,
        );
        let expected = image::imageops::crop_imm(&full, 0, 1060, 280, 200).to_image();
        let diff: u64 = preview
            .as_raw()
            .iter()
            .zip(expected.as_raw())
            .map(|(a, b)| u64::from(a.abs_diff(*b)))
            .sum();
        assert!(
            diff / (preview.as_raw().len() as u64) < 24,
            "缩略图应与整张缩小的结果相近"
        );
        // 很矮的长图：整张缩小；本来就窄的图不放大。
        let short = Stitcher::new(view(&content, 300, 0, 0, 0)).preview(500);
        assert_eq!((short.width(), short.height()), (280, 210));
        let narrow = Stitcher::new(RgbaImage::new(100, 50)).preview(500);
        assert_eq!((narrow.width(), narrow.height()), (100, 50));
    }

    /// 回放目录 `SCROLL_FRAMES` 里按文件名排序的截图（PNG），打印每一帧的匹配情况。手动排查用。
    #[test]
    #[ignore = "需要截好的画面，手动运行：SCROLL_FRAMES=<目录> cargo test replay_frames -- --ignored --nocapture"]
    fn replay_frames() {
        let dir = std::env::var("SCROLL_FRAMES").expect("设置 SCROLL_FRAMES");
        let mut paths: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
            .filter(|path| path.file_name().is_some_and(|name| name != "stitched.png"))
            .collect();
        paths.sort();
        let load = |path: &std::path::Path| image::open(path).unwrap().to_rgba8();
        let mut stitcher = Stitcher::new(load(&paths[0]));
        for path in &paths[1..] {
            let frame = load(path);
            let rows = fingerprint(&frame);
            let prev = stitcher.last_rows.clone();
            let top = prev
                .iter()
                .zip(&rows)
                .take_while(|(a, b)| a.hash == b.hash)
                .count();
            let bottom = prev
                .iter()
                .rev()
                .zip(rows.iter().rev())
                .take_while(|(a, b)| a.hash == b.hash)
                .count();
            let content = rows.iter().filter(|row| row.content).count();
            // 每个有内容的行在上一帧里任意位置有没有相同的行。
            let found = rows
                .iter()
                .filter(|row| row.content && prev.iter().any(|p| p.hash == row.hash))
                .count();
            let shift = find_shift(&prev, &rows);
            let started = std::time::Instant::now();
            let step = stitcher.push(frame);
            let _ = stitcher.preview(2400);
            let elapsed = started.elapsed();
            println!(
                "{}: top {top} bottom {bottom} content {content} found {found} {shift:?} → {step:?} {elapsed:?}",
                path.file_name().unwrap().to_string_lossy()
            );
        }
        let out = std::path::Path::new(&dir).join("stitched.png");
        stitcher.image().save(&out).unwrap();
        println!(
            "长图 {}×{} → {}",
            stitcher.width(),
            stitcher.height(),
            out.display()
        );
    }

    #[test]
    fn thumbnail_follows_truncation() {
        let mut thumbnail = Thumbnail::new(560);
        let row = |value: u8| vec![value; 560 * 4];
        let mut source: Vec<u8> = (0..10).flat_map(|i| row(i * 10)).collect();
        thumbnail.sync(&source);
        // 2 倍缩小：10 行变 5 行，每行是相邻两行的平均。
        assert_eq!(thumbnail.rows.len(), 5 * 280 * 4);
        assert_eq!(thumbnail.rows[4 * 280 * 4], 85);
        // 原图去掉 3 行（剩 7 行），再接上 3 行新的。
        source.truncate(7 * 560 * 4);
        thumbnail.sync(&source);
        assert_eq!(thumbnail.rows.len(), 3 * 280 * 4);
        source.extend((0..3).flat_map(|_| row(200)));
        thumbnail.sync(&source);
        assert_eq!(thumbnail.rows.len(), 5 * 280 * 4);
        assert_eq!(thumbnail.rows[3 * 280 * 4], 130);
        assert_eq!(thumbnail.rows[4 * 280 * 4], 200);
    }
}
