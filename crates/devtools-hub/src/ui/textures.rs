//! 缩略图与应用图标的纹理缓存。

use std::collections::HashMap;

use devtools_clipboard::Store;

use crate::platform::icons;
use egui::{ColorImage, TextureHandle, TextureId, TextureOptions};

/// 同时限制像素字节数与条目数（包括解码失败的条目），避免图片历史持续占用内存。
const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRIES: usize = 300;

struct Entry {
    texture: Option<TextureHandle>,
    bytes: usize,
    used: u64,
}

/// 图片条目的缩略图纹理，按条目 id 缓存（解码失败的也缓存为 `None`）。
#[derive(Default)]
pub struct Textures {
    map: HashMap<i64, Entry>,
    bytes: usize,
    clock: u64,
}

impl Textures {
    /// 条目缩略图的纹理，第一次用到时从数据库读出并解码。
    pub fn get(&mut self, ctx: &egui::Context, store: &Store, id: i64) -> Option<TextureHandle> {
        self.clock += 1;
        if let Some(entry) = self.map.get_mut(&id) {
            entry.used = self.clock;
            return entry.texture.clone();
        }
        let texture = store
            .thumbnail(id)
            .ok()
            .flatten()
            .and_then(|png| decode(&png))
            .map(|image| ctx.load_texture(format!("thumb-{id}"), image, TextureOptions::LINEAR));
        self.insert(id, texture.clone());
        texture
    }

    /// 超限时只淘汰最久未访问的条目，保留当前视口最近用过的缩略图。
    fn insert(&mut self, id: i64, texture: Option<TextureHandle>) {
        self.forget(id);
        self.clock += 1;
        let bytes = texture.as_ref().map_or(0, TextureHandle::byte_size);
        // 异常的大缩略图仍可供当前帧显示，但不进入缓存。
        if bytes > MAX_BYTES {
            return;
        }
        while self.bytes + bytes > MAX_BYTES || self.map.len() >= MAX_ENTRIES {
            let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, entry)| entry.used)
                .map(|(&id, _)| id)
            else {
                break;
            };
            self.forget(oldest);
        }
        self.bytes += bytes;
        self.map.insert(
            id,
            Entry {
                texture,
                bytes,
                used: self.clock,
            },
        );
    }

    /// 条目被删除时释放纹理。
    pub fn forget(&mut self, id: i64) {
        if let Some(entry) = self.map.remove(&id) {
            self.bytes -= entry.bytes;
        }
    }

    /// 返回待释放的 GPU 纹理编号；隐藏时 eframe 不绘制，需由宿主同步释放 GPU 端。
    pub fn clear(&mut self) -> Vec<TextureId> {
        let ids = self
            .map
            .values()
            .filter_map(|entry| entry.texture.as_ref().map(TextureHandle::id))
            .collect();
        self.map.clear();
        self.bytes = 0;
        ids
    }
}

/// 来源应用图标，按 bundle id 缓存（找不到的应用也缓存为 `None`，避免反复查询）。
#[derive(Default)]
pub struct AppIcons {
    map: HashMap<String, Option<TextureHandle>>,
}

impl AppIcons {
    /// 面板闲置时一并释放应用图标。
    pub fn clear(&mut self) -> Vec<TextureId> {
        let ids = self
            .map
            .values()
            .filter_map(|texture| texture.as_ref().map(TextureHandle::id))
            .collect();
        self.map.clear();
        ids
    }

    /// 应用图标的纹理，第一次用到时向系统查询。
    pub fn get(&mut self, ctx: &egui::Context, bundle_id: &str) -> Option<TextureHandle> {
        if let Some(texture) = self.map.get(bundle_id) {
            return texture.clone();
        }
        let texture = icons::app_icon_png(bundle_id, 64.0)
            .and_then(|png| decode(&png))
            .map(|image| {
                ctx.load_texture(format!("app-{bundle_id}"), image, TextureOptions::LINEAR)
            });
        self.map.insert(bundle_id.to_string(), texture.clone());
        texture
    }
}

/// 把 PNG 解码成 egui 的图片。
fn decode(png: &[u8]) -> Option<ColorImage> {
    let image = image::load_from_memory(png).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, image.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used_texture_within_byte_budget() {
        let ctx = egui::Context::default();
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut cache = Textures::default();
        let texture = |id| {
            ctx.load_texture(
                format!("test-{id}"),
                ColorImage::filled([1024, 1024], egui::Color32::WHITE),
                TextureOptions::LINEAR,
            )
        };
        // 四张各 4 MiB 的图片填满预算，重新访问第一张后应淘汰第二张。
        for id in 1..=4 {
            cache.insert(id, Some(texture(id)));
        }
        assert_eq!(cache.bytes, MAX_BYTES);
        assert!(cache.get(&ctx, &store, 1).is_some());
        cache.insert(5, Some(texture(5)));
        assert!(cache.map.contains_key(&1));
        assert!(!cache.map.contains_key(&2));
        assert!(cache.map.contains_key(&5));
        assert_eq!(cache.bytes, MAX_BYTES);

        cache.forget(1);
        assert_eq!(cache.bytes, 12 * 1024 * 1024);
        assert_eq!(cache.clear().len(), 3);
        assert_eq!(cache.bytes, 0);
        assert!(cache.map.is_empty());
    }

    #[test]
    fn failed_decodes_cannot_grow_without_bound() {
        let mut cache = Textures::default();
        for id in 0..=MAX_ENTRIES as i64 {
            cache.insert(id, None);
        }
        assert_eq!(cache.map.len(), MAX_ENTRIES);
        assert_eq!(cache.bytes, 0);
        assert!(!cache.map.contains_key(&0));
        assert!(cache.clear().is_empty());
        assert!(cache.map.is_empty());
    }
}
