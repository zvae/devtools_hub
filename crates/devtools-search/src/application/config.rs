//! 从 SearchSettings 派生 core 的 IndexConfig / QueryOptions（§2：core 不读取 UI 字段）。

use devtools_search_core::{IndexConfig, QueryOptions, RootSpec};

use crate::settings::SearchSettings;

/// 索引配置投影：只包含启用的根。
pub fn index_config(settings: &SearchSettings) -> IndexConfig {
    IndexConfig {
        roots: settings
            .roots
            .iter()
            .filter(|root| root.enabled)
            .map(|root| RootSpec {
                location: root.location.clone(),
                enabled: true,
            })
            .collect(),
        exclude_rules: settings.exclude_patterns.clone(),
        exclude_case_sensitive: settings.exclude_case_sensitive,
        ignore_paths: settings.ignore_paths.clone(),
        scan_app_bundles: settings.scan_app_bundles,
        watch: settings.watch,
    }
}

/// 查询行为投影。
pub fn query_options(settings: &SearchSettings, case_sensitive: bool) -> QueryOptions {
    QueryOptions {
        case_sensitive,
        max_results: settings.max_results.min(100_000),
        show_all_on_empty: settings.show_all_on_empty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_config_keeps_only_enabled_roots() {
        let mut settings = SearchSettings::default();
        settings.roots[0].enabled = false;
        let config = index_config(&settings);
        assert!(config.roots.is_empty());
        settings.roots[0].enabled = true;
        assert_eq!(index_config(&settings).roots.len(), 1);
        assert_eq!(config.exclude_rules, settings.exclude_patterns);
    }

    #[test]
    fn query_options_are_clamped() {
        let mut settings = SearchSettings::default();
        settings.max_results = 500_000;
        assert_eq!(query_options(&settings, true).max_results, 100_000);
        settings.max_results = 0;
        assert_eq!(query_options(&settings, false).max_results, 0);
    }
}
