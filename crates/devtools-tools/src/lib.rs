//! 目录工具集合：构建时发现；全局只保存不可变描述符，每个应用独立创建实例。

mod support;
include!(concat!(env!("OUT_DIR"), "/tools_registry.rs"));

pub fn registrations() -> &'static [devtools_module_api::ModuleDescriptor] {
    static REGISTRY: std::sync::OnceLock<Vec<devtools_module_api::ModuleDescriptor>> =
        std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut descriptors = discovered();
        descriptors.sort_by_key(|d| (d.order, d.id));
        descriptors
    })
}

pub fn descriptor(id: &str) -> Option<&'static devtools_module_api::ModuleDescriptor> {
    registrations().iter().find(|d| d.id == id)
}

#[cfg(test)]
mod test_support;
