//! 插件 manifest 的静态校验（纯函数，不涉及跨插件状态）。

use std::collections::HashSet;

use flick_domain::PluginManifest;

use crate::contract::{ActionSpec, PluginRegistrationError};

/// 校验插件 manifest 与它的动作集是否合法。
///
/// 覆盖不依赖其它插件信息的规则：空 id、空 author、setting key 重复、
/// surface id 重复、无 Action、命名空间前缀、自依赖、依赖重复声明。
///
/// `ActionId` 冲突需要注册表状态，仍由 `ActionRegistry` 负责。
pub fn validate_manifest(
    manifest: &PluginManifest,
    actions: &[ActionSpec],
) -> Result<(), PluginRegistrationError> {
    if manifest.id.trim().is_empty() {
        return Err(PluginRegistrationError::InvalidManifest {
            reason: "empty plugin id".into(),
        });
    }
    if manifest.author.trim().is_empty() {
        return Err(PluginRegistrationError::InvalidManifest {
            reason: "empty plugin author".into(),
        });
    }
    // setting key 在同一插件内必须唯一。
    let mut seen_keys = HashSet::new();
    for setting in &manifest.settings {
        if !seen_keys.insert(setting.key.as_str()) {
            return Err(PluginRegistrationError::DuplicateSettingKey {
                key: setting.key.clone(),
            });
        }
    }
    // surface id 在同一插件内必须唯一。
    let mut seen_surfaces = HashSet::new();
    for surface in &manifest.surfaces {
        if !seen_surfaces.insert(surface.id.as_str()) {
            return Err(PluginRegistrationError::SurfaceIdCollision {
                surface_id: surface.id.clone(),
            });
        }
    }
    if actions.is_empty() {
        return Err(PluginRegistrationError::NoActions);
    }
    // 所有动作必须落在插件自己的命名空间下。
    let prefix = format!("{}.", manifest.id);
    for spec in actions {
        let action_id = spec.descriptor.id.as_str();
        if !action_id.starts_with(&prefix) {
            return Err(PluginRegistrationError::NamespaceViolation {
                action_id: action_id.to_string(),
                plugin_id: manifest.id.clone(),
            });
        }
    }
    // 自依赖是纯粹的声明错误，没有合理解释空间。
    if manifest
        .plugin_dependencies
        .iter()
        .any(|dependency| dependency.plugin_id == manifest.id)
    {
        return Err(PluginRegistrationError::InvalidManifest {
            reason: "plugin depends on itself".into(),
        });
    }
    // 同一依赖被声明两次时「取哪一个 requirement」无法定义，
    // 会让解析结果依赖遍历顺序，因此直接拒绝。
    let mut seen_dependencies = HashSet::new();
    for dependency in &manifest.plugin_dependencies {
        if !seen_dependencies.insert(dependency.plugin_id.as_str()) {
            return Err(PluginRegistrationError::InvalidManifest {
                reason: "duplicate dependency".into(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flick_domain::{
        ActionDescriptor, ActionId, ConcurrencyPolicy, DependencyRequirement, PluginDependency,
        PluginKind, PluginProfile,
    };
    use std::sync::Arc;

    /// 测试用的空操作处理器。
    struct NoopHandler;

    #[async_trait::async_trait]
    impl crate::ActionHandler for NoopHandler {
        async fn invoke(
            &self,
            _req: flick_domain::ActionRequest,
            _ctx: flick_domain::ActionContext,
        ) -> Result<flick_domain::ActionOutcome, flick_domain::ActionError> {
            Ok(flick_domain::ActionOutcome::Data(serde_json::json!({})))
        }
    }

    /// 构造一个合法的插件 manifest，便于各用例只改关心的字段。
    fn manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: id.into(),
            name: "P".into(),
            author: "Flick".into(),
            version: "1.0.0".into(),
            kind: PluginKind::BuiltInRust,
            keywords: vec![],
            icon: None,
            profile: PluginProfile::default(),
            permissions: vec![],
            plugin_dependencies: vec![],
            settings: vec![],
            surfaces: vec![],
            supported_os: vec![],
            supported_architectures: vec![],
            needs_search: false,
        }
    }

    /// 构造一个属于给定插件的动作。
    fn spec(action_id: &str) -> ActionSpec {
        ActionSpec {
            descriptor: ActionDescriptor::new(
                ActionId::new(action_id).unwrap(),
                "T",
                vec!["x".into()],
                ConcurrencyPolicy::Parallel,
            ),
            handler: Arc::new(NoopHandler),
        }
    }

    #[test]
    fn empty_id_is_rejected() {
        let mut m = manifest("demo.plugin");
        m.id = "  ".into();
        match validate_manifest(&m, &[spec("demo.plugin.run")]).unwrap_err() {
            PluginRegistrationError::InvalidManifest { reason } => {
                assert!(reason.contains("empty plugin id"), "got {reason}")
            }
            e => panic!("expected invalid manifest, got {e:?}"),
        }
    }

    #[test]
    fn empty_author_is_rejected() {
        let mut m = manifest("demo.plugin");
        m.author = "   ".into();
        match validate_manifest(&m, &[spec("demo.plugin.run")]).unwrap_err() {
            PluginRegistrationError::InvalidManifest { reason } => {
                assert!(reason.contains("author"), "got {reason}")
            }
            e => panic!("expected invalid manifest, got {e:?}"),
        }
    }

    #[test]
    fn action_outside_namespace_is_rejected() {
        match validate_manifest(&manifest("demo.plugin"), &[spec("other.thing.run")]).unwrap_err() {
            PluginRegistrationError::NamespaceViolation {
                action_id,
                plugin_id,
            } => {
                assert_eq!(action_id, "other.thing.run");
                assert_eq!(plugin_id, "demo.plugin");
            }
            e => panic!("expected namespace violation, got {e:?}"),
        }
    }

    #[test]
    fn self_dependency_is_rejected() {
        let mut m = manifest("demo.plugin");
        m.plugin_dependencies = vec![PluginDependency {
            plugin_id: "demo.plugin".into(),
            requirement: DependencyRequirement::Required,
        }];
        match validate_manifest(&m, &[spec("demo.plugin.run")]).unwrap_err() {
            PluginRegistrationError::InvalidManifest { reason } => {
                assert!(reason.contains("itself"), "got {reason}")
            }
            e => panic!("expected invalid manifest, got {e:?}"),
        }
    }

    #[test]
    fn duplicate_dependency_is_rejected() {
        let mut m = manifest("demo.plugin");
        let dep = PluginDependency {
            plugin_id: "flick.builtin.apps".into(),
            requirement: DependencyRequirement::Required,
        };
        m.plugin_dependencies = vec![dep.clone(), dep];
        match validate_manifest(&m, &[spec("demo.plugin.run")]).unwrap_err() {
            PluginRegistrationError::InvalidManifest { reason } => {
                assert!(reason.contains("duplicate dependency"), "got {reason}")
            }
            e => panic!("expected invalid manifest, got {e:?}"),
        }
    }
}
