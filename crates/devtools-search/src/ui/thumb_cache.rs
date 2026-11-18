//! 缩略图纹理缓存：解码放在后台线程，界面只取已经好的纹理。
//!
//! 图标视图一屏可能同时出现几十张卡片，而图片解码是几十到几百毫秒级的操作，
//! 放在 UI 线程上滚动会一顿一顿。这里起两个专用线程处理请求：
//! 解完直接上传纹理（`egui::Context::load_texture` 允许任意线程调用）并回传句柄，
//! 下一帧界面就能画出来，期间先显示矢量图标。
//!
//! 内容预览按路径和尺寸缓存，文件图标按扩展名和尺寸复用。
//! 同类型的图标可用作占位，但内容预览要逐文件判断。

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use egui::{ColorImage, TextureHandle, TextureOptions};

use crate::{HostServices, Thumbnail};

/// 限制内容预览内存与快滚时排队的工作量。
const MAX_PREVIEWS: usize = 160;
const MAX_PREVIEW_BYTES: usize = 24 * 1024 * 1024;
const MAX_ICONS: usize = 64;
const MAX_PENDING: usize = 32;
/// 解码线程数（每个线程是一个独立的 COM 单元）。
const WORKERS: usize = 2;

/// 一次请求的键：路径 + 目标尺寸（三种图标视图尺寸不同）。
type RequestKey = (PathBuf, u32);

struct Reply {
    key: RequestKey,
    is_dir: bool,
    /// 这次拿到的是图标时，按扩展名复用的键。
    icon_key: Option<String>,
    texture: Option<TextureHandle>,
}

/// 缩略图缓存（只在 UI 线程访问）。
#[derive(Default)]
pub struct ThumbnailCache {
    previews: HashMap<RequestKey, Option<TextureHandle>>,
    preview_order: VecDeque<RequestKey>,
    preview_bytes: usize,
    icons: HashMap<String, TextureHandle>,
    /// 已经排进后台队列、还没回来的请求。
    pending: HashSet<RequestKey>,
    /// 每个解码线程一个队列（两个 worker 各自 `recv`，不用共享接收端）。
    queues: Vec<Sender<(RequestKey, bool)>>,
    /// 下一次投递用哪个队列（轮流，避免全压在一个线程上）。
    next_queue: usize,
    replies: Option<Receiver<Reply>>,
    closed: Arc<AtomicBool>,
}

impl ThumbnailCache {
    /// 取缩略图纹理。还没解好时返回 `None`（调用方先画矢量图标），同时排一个后台请求。
    ///
    /// `is_dir` 用来把图标按"目录 / 扩展名"归组复用。
    pub fn texture(
        &mut self,
        ctx: &egui::Context,
        host: &Arc<dyn HostServices>,
        path: &str,
        size: u32,
        is_dir: bool,
    ) -> Option<TextureHandle> {
        self.drain();
        let key = (PathBuf::from(path), size);
        let key_of_icon = icon_key(Path::new(path), size, is_dir);
        let fallback = self.icons.get(&key_of_icon).cloned();
        if let Some(cached) = self.previews.get(&key) {
            return cached.clone().or(fallback);
        }
        if is_dir && fallback.is_some() {
            return fallback;
        }
        if self.pending.contains(&key) || self.pending.len() >= MAX_PENDING {
            return fallback;
        }
        if !self.ensure_worker(ctx, host) {
            return fallback;
        }
        let sender = &self.queues[self.next_queue % self.queues.len()];
        self.next_queue = self.next_queue.wrapping_add(1);
        if sender.send((key.clone(), is_dir)).is_ok() {
            self.pending.insert(key);
        }
        fallback
    }

    /// 只读缓存：拖动框选时代替 [`texture`](Self::texture)，不发起新的解码请求
    /// （解码线程抢 CPU 会让拖动发飘）。
    pub fn cached(&self, path: &str, size: u32, is_dir: bool) -> Option<TextureHandle> {
        let key = (PathBuf::from(path), size);
        self.previews
            .get(&key)
            .and_then(|texture| texture.clone())
            .or_else(|| {
                self.icons
                    .get(&icon_key(Path::new(path), size, is_dir))
                    .cloned()
            })
    }

    /// 把后台已经做完的结果收进缓存。
    fn drain(&mut self) {
        while let Some(reply) = self
            .replies
            .as_ref()
            .and_then(|replies| replies.try_recv().ok())
        {
            self.pending.remove(&reply.key);
            let preview = match reply.icon_key {
                Some(icon_key) => {
                    if let Some(texture) = reply.texture {
                        if self.icons.len() >= MAX_ICONS && !self.icons.contains_key(&icon_key) {
                            self.icons.clear();
                        }
                        self.icons.entry(icon_key).or_insert(texture);
                        if reply.is_dir {
                            continue;
                        }
                    }
                    None
                }
                None => reply.texture,
            };
            let bytes = preview.as_ref().map_or(0, TextureHandle::byte_size);
            while self.previews.len() >= MAX_PREVIEWS
                || self.preview_bytes + bytes > MAX_PREVIEW_BYTES
            {
                let Some(oldest) = self.preview_order.pop_front() else {
                    break;
                };
                if let Some(Some(texture)) = self.previews.remove(&oldest) {
                    self.preview_bytes -= texture.byte_size();
                }
            }
            if bytes <= MAX_PREVIEW_BYTES {
                self.preview_bytes += bytes;
                self.preview_order.push_back(reply.key.clone());
                self.previews.insert(reply.key, preview);
            }
        }
    }

    /// 第一次用到时启动解码线程（失败就永久回退到矢量图标）。
    fn ensure_worker(&mut self, ctx: &egui::Context, host: &Arc<dyn HostServices>) -> bool {
        if !self.queues.is_empty() {
            return true;
        }
        let (reply_tx, reply_rx) = channel::<Reply>();
        for index in 0..WORKERS {
            let (work_tx, work_rx) = channel::<(RequestKey, bool)>();
            let reply_tx = reply_tx.clone();
            let ctx = ctx.clone();
            let host = host.clone();
            let closed = self.closed.clone();
            let ok = std::thread::Builder::new()
                .name(format!("search-thumbnails-{index}"))
                .spawn(move || {
                    while let Ok((key, is_dir)) = work_rx.recv() {
                        if closed.load(Ordering::Relaxed) {
                            break;
                        }
                        let thumbnail = host.thumbnail(&key.0, key.1);
                        if closed.load(Ordering::Relaxed) {
                            break;
                        }
                        let icon_key = thumbnail
                            .as_ref()
                            .filter(|thumbnail| !thumbnail.preview)
                            .map(|_| icon_key(&key.0, key.1, is_dir));
                        let texture = thumbnail.and_then(|thumbnail| upload(&ctx, &key, thumbnail));
                        if reply_tx
                            .send(Reply {
                                key,
                                is_dir,
                                icon_key,
                                texture,
                            })
                            .is_err()
                        {
                            break;
                        }
                        // 结果到货后让界面重画一帧，不然要等用户下一次操作才显示。
                        host.request_repaint();
                    }
                })
                .is_ok();
            if ok {
                self.queues.push(work_tx);
            }
        }
        if self.queues.is_empty() {
            return false;
        }
        self.replies = Some(reply_rx);
        true
    }
}

impl Drop for ThumbnailCache {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

/// 图标复用的键：目录一张、每种扩展名一张。
fn icon_key(path: &Path, size: u32, is_dir: bool) -> String {
    if is_dir {
        return format!("{size}:<dir>");
    }
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    match extension {
        Some(ext) if !ext.is_empty() => format!("{size}:{ext}"),
        _ => format!("{size}:<none>"),
    }
}

/// 像素 → GPU 纹理（在后台线程上传，egui 允许）。
fn upload(ctx: &egui::Context, key: &RequestKey, thumbnail: Thumbnail) -> Option<TextureHandle> {
    let expected = (thumbnail.width as usize) * (thumbnail.height as usize) * 4;
    if thumbnail.width == 0 || thumbnail.height == 0 || thumbnail.rgba.len() < expected {
        return None;
    }
    let image = ColorImage::from_rgba_unmultiplied(
        [thumbnail.width as usize, thumbnail.height as usize],
        &thumbnail.rgba[..expected],
    );
    Some(ctx.load_texture(
        format!("search-thumb:{}:{}", key.1, key.0.display()),
        image,
        TextureOptions::LINEAR,
    ))
}
