//! 原生文件图标的纹理缓存。
//!
//! 图标的取用按"类型"缓存：目录一张、每种扩展名一张（`.pdf`、`.app`…），
//! 避免每帧对每一行调用系统 API。取不到原生图标时缓存 `None`，界面回退到矢量图标。

use std::collections::HashMap;
use std::path::Path;

use devtools_search_core::NodeKind;
use egui::{ColorImage, TextureHandle, TextureOptions};

use crate::HostServices;

/// 缓存条目上限（扩展名很多时也不无限增长）。
const MAX_ENTRIES: usize = 128;

#[derive(Default)]
pub struct IconCache {
    textures: HashMap<String, Option<TextureHandle>>,
}

impl IconCache {
    /// 取某个结果行的图标纹理（必要时向宿主请求并上传）。
    pub fn icon(
        &mut self,
        ctx: &egui::Context,
        host: &dyn HostServices,
        kind: NodeKind,
        name: &str,
        path: &Path,
    ) -> Option<TextureHandle> {
        let key = match kind {
            NodeKind::Dir => "dir".to_string(),
            NodeKind::File => match name.rsplit_once('.') {
                // 隐藏文件（`.gitignore`）没有扩展名。
                Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
                    format!("file:{}", ext.to_ascii_lowercase())
                }
                _ => "file".to_string(),
            },
        };
        if let Some(cached) = self.textures.get(&key) {
            return cached.clone();
        }
        let texture = host.icon(path).and_then(|(width, height, rgba)| {
            if width == 0 || height == 0 || rgba.len() < (width * height * 4) as usize {
                return None;
            }
            let image = ColorImage::from_rgba_unmultiplied(
                [width as usize, height as usize],
                &rgba[..(width * height * 4) as usize],
            );
            Some(ctx.load_texture(format!("search-icon:{key}"), image, TextureOptions::LINEAR))
        });
        if self.textures.len() >= MAX_ENTRIES {
            self.textures.clear();
        }
        self.textures.insert(key, texture.clone());
        texture
    }
}
