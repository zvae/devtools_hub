//! 缩略图与应用图标的纹理缓存。

use std::collections::HashMap;

use devtools_clipboard::Store;

use crate::platform::icons;
use egui::{ColorImage, TextureHandle, TextureOptions};

/// 缓存的纹理数上限，超过后整个清空重建。
const MAX_ENTRIES: usize = 300;

/// 图片条目的缩略图纹理，按条目 id 缓存（解码失败的也缓存为 `None`）。
#[derive(Default)]
pub struct Textures {
    map: HashMap<i64, Option<TextureHandle>>,
}

impl Textures {
    /// 条目缩略图的纹理，第一次用到时从数据库读出并解码。
    pub fn get(&mut self, ctx: &egui::Context, store: &Store, id: i64) -> Option<TextureHandle> {
        if let Some(texture) = self.map.get(&id) {
            return texture.clone();
        }
        if self.map.len() >= MAX_ENTRIES {
            self.map.clear();
        }
        let texture = store
            .thumbnail(id)
            .ok()
            .flatten()
            .and_then(|png| decode(&png))
            .map(|image| ctx.load_texture(format!("thumb-{id}"), image, TextureOptions::LINEAR));
        self.map.insert(id, texture.clone());
        texture
    }

    /// 条目被删除时释放纹理。
    pub fn forget(&mut self, id: i64) {
        self.map.remove(&id);
    }
}

/// 来源应用图标，按 bundle id 缓存（找不到的应用也缓存为 `None`，避免反复查询）。
#[derive(Default)]
pub struct AppIcons {
    map: HashMap<String, Option<TextureHandle>>,
}

impl AppIcons {
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
