//! 插件依赖解析：纯函数，不持有注册表、不做 I/O。

use std::collections::{BTreeMap, HashMap, HashSet};

use flick_domain::{DependencyRequirement, DependencyStatus, PluginManifest};

use crate::contract::PluginRegistrationError;

/// 依赖解析的完整结果。
///
/// 一次性返回「每个插件的逐条依赖状态」「不可用插件集合」与
/// 「全部缺失错误」，让调用方既能把结果应用到注册表，又能
/// 完整上报问题（不短路只报第一个）。
#[derive(Debug, Clone, Default)]
pub struct DependencyResolution {
    /// 插件 id → 该插件声明的每条依赖的解析状态。
    pub statuses: HashMap<String, Vec<DependencyStatus>>,
    /// 因必需依赖（含传递）不满足而不可用的插件 id。
    pub unavailable: HashSet<String>,
    /// 全部 `DependencyMissing` 错误。
    pub errors: Vec<PluginRegistrationError>,
}

/// 解析一组已注册 manifest 之间的依赖关系。
///
/// 用 `BTreeMap` 而非 `HashMap` 作为输入，是为了让迭代顺序确定——
/// 测试断言与 `flick doctor` 输出都依赖这一点。
///
/// 语义：
/// - `Required` 依赖的插件不存在、或它自己不可用 → 本插件不可用；
/// - `Optional` 依赖缺失只体现为 `available == false`，不影响可用性；
/// - 环（A→B→A）不报错：依赖只决定可用性，不决定加载顺序。
pub fn resolve_dependencies(manifests: &BTreeMap<String, PluginManifest>) -> DependencyResolution {
    let mut unavailable: HashSet<String> = HashSet::new();
    // 失效传播最长链不超过插件数，多跑一轮用于确认「无变化」。
    let max_rounds = manifests.len() + 1;
    for _ in 0..max_rounds {
        let mut changed = false;
        for (plugin_id, manifest) in manifests {
            if unavailable.contains(plugin_id) {
                continue;
            }
            if has_broken_required(manifest, manifests, &unavailable) {
                unavailable.insert(plugin_id.clone());
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut statuses = HashMap::with_capacity(manifests.len());
    let mut errors = Vec::new();
    for (plugin_id, manifest) in manifests {
        let per_dependency: Vec<DependencyStatus> = manifest
            .plugin_dependencies
            .iter()
            .map(|dependency| DependencyStatus {
                plugin_id: dependency.plugin_id.clone(),
                requirement: dependency.requirement,
                available: manifests.contains_key(&dependency.plugin_id)
                    && !unavailable.contains(&dependency.plugin_id),
            })
            .collect();
        // 不可用插件至少有一条未满足的必需依赖，报一条即可定位问题。
        if unavailable.contains(plugin_id) {
            errors.push(PluginRegistrationError::DependencyMissing {
                plugin_id: plugin_id.clone(),
            });
        }
        statuses.insert(plugin_id.clone(), per_dependency);
    }

    DependencyResolution {
        statuses,
        unavailable,
        errors,
    }
}

/// 判断某插件是否有一条必需依赖未满足（不存在或自身不可用）。
fn has_broken_required(
    manifest: &PluginManifest,
    manifests: &BTreeMap<String, PluginManifest>,
    unavailable: &HashSet<String>,
) -> bool {
    manifest.plugin_dependencies.iter().any(|dependency| {
        dependency.requirement == DependencyRequirement::Required
            && (!manifests.contains_key(&dependency.plugin_id)
                || unavailable.contains(&dependency.plugin_id))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::PluginRegistrationError;
    use flick_domain::{PluginDependency, PluginKind, PluginProfile};

    /// 构造一个只带依赖声明的最小 manifest。
    fn manifest(id: &str, dependencies: Vec<(&str, DependencyRequirement)>) -> PluginManifest {
        PluginManifest {
            id: id.into(),
            name: id.into(),
            author: "Flick".into(),
            version: "1.0.0".into(),
            kind: PluginKind::BuiltInRust,
            keywords: vec![],
            icon: None,
            profile: PluginProfile::default(),
            permissions: vec![],
            plugin_dependencies: dependencies
                .into_iter()
                .map(|(plugin_id, requirement)| PluginDependency {
                    plugin_id: plugin_id.into(),
                    requirement,
                })
                .collect(),
            settings: vec![],
            surfaces: vec![],
            supported_os: vec![],
            supported_architectures: vec![],
            needs_search: false,
        }
    }

    /// 把 (id, 依赖) 列表装配成解析函数需要的 `BTreeMap`。
    fn registry(
        entries: Vec<(&str, Vec<(&str, DependencyRequirement)>)>,
    ) -> BTreeMap<String, PluginManifest> {
        entries
            .into_iter()
            .map(|(id, deps)| (id.to_string(), manifest(id, deps)))
            .collect()
    }

    #[test]
    fn required_dependency_present_is_available() {
        let resolution = resolve_dependencies(&registry(vec![
            ("demo.a", vec![("demo.b", DependencyRequirement::Required)]),
            ("demo.b", vec![]),
        ]));
        assert!(resolution.unavailable.is_empty());
        assert!(resolution.errors.is_empty());
        let statuses = &resolution.statuses["demo.a"];
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].plugin_id, "demo.b");
        assert!(statuses[0].available);
    }

    #[test]
    fn required_dependency_missing_marks_unavailable() {
        let resolution = resolve_dependencies(&registry(vec![(
            "demo.a",
            vec![("demo.missing", DependencyRequirement::Required)],
        )]));
        assert_eq!(resolution.unavailable.len(), 1);
        assert!(resolution.unavailable.contains("demo.a"));
        assert_eq!(resolution.errors.len(), 1);
        assert!(matches!(
            &resolution.errors[0],
            PluginRegistrationError::DependencyMissing { plugin_id } if plugin_id == "demo.a"
        ));
        assert!(!resolution.statuses["demo.a"][0].available);
    }

    #[test]
    fn optional_dependency_missing_keeps_available() {
        let resolution = resolve_dependencies(&registry(vec![(
            "demo.a",
            vec![("demo.missing", DependencyRequirement::Optional)],
        )]));
        assert!(resolution.unavailable.is_empty());
        assert!(resolution.errors.is_empty());
        assert!(!resolution.statuses["demo.a"][0].available);
    }

    #[test]
    fn transitive_required_failure_disables_dependents() {
        let resolution = resolve_dependencies(&registry(vec![
            ("demo.a", vec![("demo.b", DependencyRequirement::Required)]),
            ("demo.b", vec![("demo.c", DependencyRequirement::Required)]),
        ]));
        assert_eq!(resolution.unavailable.len(), 2);
        assert!(resolution.unavailable.contains("demo.a"));
        assert!(resolution.unavailable.contains("demo.b"));
        assert_eq!(resolution.errors.len(), 2);
    }

    #[test]
    fn dependency_order_does_not_matter() {
        let forward = resolve_dependencies(&registry(vec![
            ("demo.a", vec![("demo.b", DependencyRequirement::Required)]),
            ("demo.b", vec![]),
        ]));
        let backward = resolve_dependencies(&registry(vec![
            ("demo.b", vec![]),
            ("demo.a", vec![("demo.b", DependencyRequirement::Required)]),
        ]));
        assert_eq!(forward.unavailable, backward.unavailable);
        assert_eq!(forward.statuses["demo.a"], backward.statuses["demo.a"]);
    }

    #[test]
    fn cycle_between_available_plugins_is_allowed() {
        let resolution = resolve_dependencies(&registry(vec![
            ("demo.a", vec![("demo.b", DependencyRequirement::Required)]),
            ("demo.b", vec![("demo.a", DependencyRequirement::Required)]),
        ]));
        assert!(resolution.unavailable.is_empty());
        assert!(resolution.errors.is_empty());
    }
}
