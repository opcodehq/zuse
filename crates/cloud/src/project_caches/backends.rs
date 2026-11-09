//! Preparation adapters are explicitly registered per provider. Ordinary machine
//! support or a database capability row never supplies a missing cache adapter.
use super::{source::Bundles, worker::*, *};
use async_trait::async_trait;
use std::{collections::BTreeMap, sync::Arc};
use zuse_protocol::cloud_computers::ProviderId;

pub(crate) struct Registry {
    providers: BTreeMap<ProviderId, Arc<dyn PreparationBackend>>,
    bundles: Arc<dyn Bundles>,
}
impl Registry {
    pub fn new(bundles: Arc<dyn Bundles>) -> Self {
        Self {
            providers: BTreeMap::new(),
            bundles,
        }
    }
    pub fn register(
        &mut self,
        provider: ProviderId,
        backend: Arc<dyn PreparationBackend>,
    ) -> Result<(), ApiError> {
        if self.providers.contains_key(&provider) {
            return Err(ApiError::conflict(
                "project_cache_provider_already_configured",
            ));
        }
        self.providers.insert(provider, backend);
        Ok(())
    }
}
#[async_trait]
impl PreparationBackend for Registry {
    async fn reap_source(&self, now: i64) -> Result<bool, ApiError> {
        self.bundles.reap(now).await
    }
    async fn resolve(&self, request: &Request) -> Result<Vec<PreparationPlan>, ApiError> {
        let mut plans = Vec::new();
        for (provider, backend) in &self.providers {
            for plan in backend.resolve(request).await? {
                if plan.identity.provider != *provider {
                    return Err(ApiError::conflict("project_cache_provider_scope_mismatch"));
                }
                plans.push(plan);
            }
        }
        Ok(plans)
    }
    async fn step(&self, operation: &Operation, mode: StepMode) -> Result<StepResult, ApiError> {
        self.providers
            .get(&operation.plan.identity.provider)
            .ok_or_else(|| ApiError::unavailable("project_cache_provider_not_supported"))?
            .step(operation, mode)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Source;
    #[async_trait]
    impl Bundles for Source {
        async fn resolve(&self, _: &Request) -> Result<super::super::source::Bundle, ApiError> {
            panic!("not a source-read test")
        }
        async fn load(&self, _: &str, _: &str, _: &str) -> Result<Vec<u8>, ApiError> {
            panic!("not a source-read test")
        }
    }
    struct Backend {
        calls: AtomicUsize,
        plan: PreparationPlan,
    }
    #[async_trait]
    impl PreparationBackend for Backend {
        async fn resolve(&self, _: &Request) -> Result<Vec<PreparationPlan>, ApiError> {
            Ok(vec![self.plan.clone()])
        }
        async fn step(&self, _: &Operation, _: StepMode) -> Result<StepResult, ApiError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(StepResult::Pending(json!({})))
        }
    }
    fn operation() -> Operation {
        let recipe = zuse_protocol::cloud_computers::EnvironmentRecipe::default();
        use sha2::Digest;
        Operation {
            operation_id: "cacheop_registry".into(),
            request: Request {
                request_id: "prep_registry".into(),
                owner: "owner".into(),
                actor_id: "owner".into(),
                actor: Value::Null,
                project: "project".into(),
                environment_revision: 1,
                requested_ref: "main".into(),
                repository_identity: "github.com/example/repo".into(),
            },
            plan: PreparationPlan {
                identity: CacheIdentity {
                    version: 1,
                    owner_id: "owner".into(),
                    project_id: "project".into(),
                    repository_identity: "github.com/example/repo".into(),
                    provider: ProviderId::Boxd,
                    architecture: "x86_64".into(),
                    base_release_id: "base".into(),
                    base_artifact_id: "base-artifact".into(),
                    base_artifact_version: Some("1".into()),
                    base_runtime_sha256: "a".repeat(64),
                    recipe_source: "project".into(),
                    recipe_digest: format!(
                        "{:x}",
                        sha2::Sha256::digest(serde_json::to_vec(&recipe).unwrap())
                    ),
                    source_commit: "c".repeat(40),
                    dependency_inputs: Default::default(),
                    staging_pilot_sha256: None,
                },
                recipe,
                base_artifact_binding: None,
                initial_runtime_bootstrap: None,
                source_bundle_sha256: "e".repeat(64),
            },
            phase: "cleanup".into(),
            resources: json!({}),
        }
    }
    #[tokio::test]
    async fn provider_operation_never_falls_back_to_a_different_adapter() {
        let mut operation = operation();
        let backend = Arc::new(Backend {
            calls: AtomicUsize::new(0),
            plan: operation.plan.clone(),
        });
        let mut registry = Registry::new(Arc::new(Source));
        registry
            .register(ProviderId::Boxd, backend.clone())
            .unwrap();
        assert!(
            registry
                .register(ProviderId::Boxd, backend.clone())
                .is_err()
        );
        for provider in [ProviderId::E2b, ProviderId::Boat, ProviderId::Daytona] {
            operation.plan.identity.provider = provider;
            assert_eq!(
                registry
                    .step(&operation, StepMode::Reconcile)
                    .await
                    .err()
                    .unwrap()
                    .code,
                "project_cache_provider_not_supported"
            );
        }
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        operation.plan.identity.provider = ProviderId::Boxd;
        registry
            .step(&operation, StepMode::Reconcile)
            .await
            .unwrap();
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        // Even a configured adapter cannot publish another provider's plans.
        let mut wrong = operation.plan.clone();
        wrong.identity.provider = ProviderId::Boat;
        let mut broken = Registry::new(Arc::new(Source));
        broken
            .register(
                ProviderId::Boxd,
                Arc::new(Backend {
                    calls: AtomicUsize::new(0),
                    plan: wrong,
                }),
            )
            .unwrap();
        assert_eq!(
            broken.resolve(&operation.request).await.err().unwrap().code,
            "project_cache_provider_scope_mismatch"
        );
    }
}
