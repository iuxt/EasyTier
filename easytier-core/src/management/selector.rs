use std::sync::Arc;

use easytier_proto::api::instance::{InstanceIdentifier, instance_identifier::Selector};

use crate::instance::manager::{InstanceFactory, InstanceManager};

/// Transport-independent selector for one managed Instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagementSelector {
    Id(uuid::Uuid),
    UnambiguousDefault,
}

/// Resolves one stateless management selector through the canonical Manager.
pub fn resolve_instance<F>(
    manager: &InstanceManager<F>,
    identifier: Option<&InstanceIdentifier>,
) -> anyhow::Result<Arc<F::Instance>>
where
    F: InstanceFactory,
{
    let selector = match identifier.and_then(|identifier| identifier.selector.as_ref()) {
        Some(Selector::Id(instance_id)) => ManagementSelector::Id((*instance_id).into()),
        None => ManagementSelector::UnambiguousDefault,
    };
    resolve_management_instance(manager, &selector)
}

/// Resolves one Instance without coupling the caller to an RPC request type.
pub fn resolve_management_instance<F>(
    manager: &InstanceManager<F>,
    selector: &ManagementSelector,
) -> anyhow::Result<Arc<F::Instance>>
where
    F: InstanceFactory,
{
    if let ManagementSelector::Id(instance_id) = selector {
        return manager
            .get(*instance_id)
            .ok_or_else(|| anyhow::anyhow!("Instance not found"));
    }

    let instances = manager.list();
    match instances.as_slice() {
        [] => anyhow::bail!("No instance matches the selector"),
        [instance] => Ok(instance.clone()),
        _ => anyhow::bail!(
            "{} instances match the selector, please specify the instance ID",
            instances.len()
        ),
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::{
        config::toml::{ConfigLoader as _, TomlConfig},
        instance::manager::ManagedInstance,
    };

    #[derive(Debug)]
    struct TestInstance {
        id: Uuid,
    }

    impl ManagedInstance for TestInstance {
        fn instance_id(&self) -> Uuid {
            self.id
        }
    }

    struct TestFactory;

    impl InstanceFactory for TestFactory {
        type Instance = TestInstance;
        type CreateContext = ();
        type Error = std::convert::Infallible;

        fn create(
            &self,
            config: TomlConfig,
            (): Self::CreateContext,
        ) -> Result<Arc<Self::Instance>, Self::Error> {
            Ok(Arc::new(TestInstance {
                id: config.get_id(),
            }))
        }
    }

    fn add(manager: &InstanceManager<TestFactory>, id: Uuid) {
        let config = TomlConfig::default();
        config.set_id(id);
        manager.create(config, ()).unwrap();
    }

    fn by_id(id: Uuid) -> InstanceIdentifier {
        InstanceIdentifier {
            selector: Some(Selector::Id(id.into())),
        }
    }

    #[test]
    fn resolves_uuid_and_single_implicit_instance() {
        let manager = InstanceManager::new(TestFactory, None);
        let id = Uuid::new_v4();
        add(&manager, id);

        assert_eq!(resolve_instance(&manager, Some(&by_id(id))).unwrap().id, id);
        assert_eq!(resolve_instance(&manager, None).unwrap().id, id);
        assert_eq!(
            resolve_instance(&manager, Some(&InstanceIdentifier::default()))
                .unwrap()
                .id,
            id
        );
    }

    #[test]
    fn rejects_missing_and_ambiguous_selectors() {
        let manager = InstanceManager::new(TestFactory, None);
        assert_eq!(
            resolve_instance(&manager, None).unwrap_err().to_string(),
            "No instance matches the selector"
        );
        assert_eq!(
            resolve_instance(&manager, Some(&by_id(Uuid::new_v4())))
                .unwrap_err()
                .to_string(),
            "Instance not found"
        );

        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        add(&manager, first);
        add(&manager, second);
        assert!(
            resolve_instance(&manager, None)
                .unwrap_err()
                .to_string()
                .contains("2 instances match")
        );
        assert_eq!(
            resolve_instance(&manager, Some(&by_id(first))).unwrap().id,
            first
        );
        assert_eq!(
            resolve_instance(&manager, Some(&by_id(second))).unwrap().id,
            second
        );
    }
}
