//! Deterministic provider/model routing for SC Node.
//!
//! This module is intentionally decoupled from `sc-config` and from the
//! [`crate::Provider`] trait: it only deals in plain data so it can be unit
//! tested without any I/O, HTTP client, or process wiring. Integration
//! (translating the real on-disk config and the actually-constructed
//! provider set into these types) happens elsewhere.
//!
//! Resolution priority (first satisfied step decides the outcome; a step
//! that is "satisfied" either resolves a route or fails with a typed
//! error - later steps are never used as a silent fallback for a failed
//! earlier step):
//!
//! 1. An explicit, approved provider (optionally with an explicit model).
//! 2. The first [`RoutingRule`] whose keywords match the request's task text.
//! 3. The configured fallback route, if present and enabled.
//! 4. The first enabled, credentialed, *local* provider in the supplied list.
//! 5. Otherwise, [`RoutingError::NoRouteAvailable`].
//!
//! A cloud (non-local) provider is only ever selected if `allow_cloud` is
//! `true`. This is a hard gate: if a step would otherwise select a cloud
//! provider and `allow_cloud` is `false`, resolution stops with
//! [`RoutingError::CloudNotAllowed`] rather than silently trying the next
//! step.

use thiserror::Error;

/// A provider as known to the router: just enough information to decide
/// whether it is a legal routing target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableProvider {
    /// Stable provider identifier, e.g. `"ollama"`, `"nvidia"`, `"openrouter"`.
    pub provider: String,
    /// Whether the provider is administratively enabled.
    pub enabled: bool,
    /// Whether the provider has the credentials it needs configured
    /// (e.g. an API key present). Local providers that need no key should
    /// set this to `true`.
    pub has_key: bool,
    /// Whether the provider runs entirely on the local machine (no
    /// third-party network egress of prompt/response data).
    pub is_local: bool,
}

impl AvailableProvider {
    pub fn new(provider: impl Into<String>, enabled: bool, has_key: bool, is_local: bool) -> Self {
        Self {
            provider: provider.into(),
            enabled,
            has_key,
            is_local,
        }
    }
}

/// A single deterministic routing rule: if the request's task text
/// contains any of `match_contains` (case-insensitive), this rule routes
/// to `provider`/`model`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingRule {
    pub name: String,
    pub match_contains: Vec<String>,
    pub provider: String,
    pub model: String,
}

/// The route used when no explicit override and no rule matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackRoute {
    pub provider: String,
    pub model: String,
    pub enabled: bool,
}

/// Routing configuration: ordered rules plus an optional fallback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutingConfig {
    /// Evaluated in order; the first matching rule wins.
    pub rules: Vec<RoutingRule>,
    /// Used if no rule matches. `None` behaves like a disabled fallback.
    pub fallback: Option<FallbackRoute>,
}

/// The provider/model the caller is explicitly asking for, plus optional
/// task text used to evaluate [`RoutingRule`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteRequest {
    /// An explicit provider override. When present, this always takes
    /// priority over rules/fallback/local-first - it either resolves or
    /// fails outright.
    pub requested_provider: Option<String>,
    /// An explicit model slug. Passed through unchanged to the resolved
    /// route. When absent, an empty model slug is returned, which
    /// downstream provider code treats as "use the provider's own
    /// default model" (matching the existing provider crates' convention).
    pub requested_model: Option<String>,
    /// Free-form task/query text evaluated against rule keywords.
    pub task: Option<String>,
}

/// Why a particular provider/model was selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteReason {
    /// The caller explicitly requested this provider.
    ExplicitOverride,
    /// A configured rule matched the task text (rule name).
    RuleMatch(String),
    /// The configured fallback route was used.
    Fallback,
    /// No rule/fallback applied; the first enabled local provider was used.
    LocalFirst,
}

/// A fully resolved route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoute {
    pub provider: String,
    pub model: String,
    pub reason: RouteReason,
}

/// Deterministic, typed routing failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RoutingError {
    #[error("provider '{0}' is not in the list of available providers")]
    ProviderNotFound(String),

    #[error("provider '{0}' is disabled")]
    ProviderDisabled(String),

    #[error("provider '{0}' has no credentials configured")]
    ProviderMissingKey(String),

    #[error("cloud provider '{0}' is not allowed by the current data-boundary policy")]
    CloudNotAllowed(String),

    #[error("'{0}' is not a valid model identifier")]
    InvalidModelSlug(String),

    #[error(
        "no route could be resolved: no rule matched, no fallback configured, and no enabled local provider is available"
    )]
    NoRouteAvailable,
}

/// Resolve a provider/model route deterministically. See the module docs
/// for the exact priority order and gating rules.
pub fn resolve_route(
    providers: &[AvailableProvider],
    config: &RoutingConfig,
    request: &RouteRequest,
    allow_cloud: bool,
) -> Result<ResolvedRoute, RoutingError> {
    // 1. Explicit, approved provider override.
    if let Some(provider_key) = &request.requested_provider {
        let model = request.requested_model.clone().unwrap_or_default();
        reject_default_model(&model)?;

        let provider = find_provider(providers, provider_key)?;
        validate_provider(provider, allow_cloud)?;

        return Ok(ResolvedRoute {
            provider: provider_key.clone(),
            model,
            reason: RouteReason::ExplicitOverride,
        });
    }

    // 2. First matching config rule.
    for rule in &config.rules {
        if rule_matches(rule, request.task.as_deref()) {
            reject_default_model(&rule.model)?;

            let provider = find_provider(providers, &rule.provider)?;
            validate_provider(provider, allow_cloud)?;

            return Ok(ResolvedRoute {
                provider: rule.provider.clone(),
                model: rule.model.clone(),
                reason: RouteReason::RuleMatch(rule.name.clone()),
            });
        }
    }

    // 3. Configured fallback, if enabled.
    if let Some(fallback) = &config.fallback
        && fallback.enabled
    {
        reject_default_model(&fallback.model)?;

        let provider = find_provider(providers, &fallback.provider)?;
        validate_provider(provider, allow_cloud)?;

        return Ok(ResolvedRoute {
            provider: fallback.provider.clone(),
            model: fallback.model.clone(),
            reason: RouteReason::Fallback,
        });
    }

    // 4. First enabled, credentialed, local provider.
    if let Some(provider) = providers
        .iter()
        .find(|p| p.is_local && p.enabled && p.has_key)
    {
        let model = request.requested_model.clone().unwrap_or_default();
        reject_default_model(&model)?;

        return Ok(ResolvedRoute {
            provider: provider.provider.clone(),
            model,
            reason: RouteReason::LocalFirst,
        });
    }

    // 5. Nothing usable.
    Err(RoutingError::NoRouteAvailable)
}

fn find_provider<'a>(
    providers: &'a [AvailableProvider],
    key: &str,
) -> Result<&'a AvailableProvider, RoutingError> {
    providers
        .iter()
        .find(|p| p.provider == key)
        .ok_or_else(|| RoutingError::ProviderNotFound(key.to_string()))
}

fn validate_provider(provider: &AvailableProvider, allow_cloud: bool) -> Result<(), RoutingError> {
    if !provider.enabled {
        return Err(RoutingError::ProviderDisabled(provider.provider.clone()));
    }
    if !provider.has_key {
        return Err(RoutingError::ProviderMissingKey(provider.provider.clone()));
    }
    if !provider.is_local && !allow_cloud {
        return Err(RoutingError::CloudNotAllowed(provider.provider.clone()));
    }
    Ok(())
}

/// A model slug that is literally "default" (any case, surrounding
/// whitespace ignored) is never a real model identifier - reject it so
/// callers cannot accidentally route with a meaningless placeholder.
/// An empty slug is a different, deliberate convention meaning "let the
/// provider pick its own default" and is left untouched.
fn reject_default_model(model: &str) -> Result<(), RoutingError> {
    if model.trim().eq_ignore_ascii_case("default") {
        Err(RoutingError::InvalidModelSlug(model.to_string()))
    } else {
        Ok(())
    }
}

fn rule_matches(rule: &RoutingRule, task: Option<&str>) -> bool {
    let Some(task) = task else {
        return false;
    };
    let task_lower = task.to_lowercase();
    rule.match_contains
        .iter()
        .any(|kw| task_lower.contains(&kw.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_and_research_config() -> RoutingConfig {
        RoutingConfig {
            rules: vec![
                RoutingRule {
                    name: "code-tasks".into(),
                    match_contains: vec!["code".into(), "rust".into(), "cargo".into()],
                    provider: "ollama".into(),
                    model: "codellama:7b".into(),
                },
                RoutingRule {
                    name: "research-tasks".into(),
                    match_contains: vec!["research".into(), "search".into()],
                    provider: "openrouter".into(),
                    model: "perplexity/sonar-pro".into(),
                },
            ],
            fallback: None,
        }
    }

    fn ollama_local(enabled: bool) -> AvailableProvider {
        AvailableProvider::new("ollama", enabled, true, true)
    }

    fn openrouter_cloud(enabled: bool) -> AvailableProvider {
        AvailableProvider::new("openrouter", enabled, true, false)
    }

    #[test]
    fn local_code_route() {
        let providers = vec![ollama_local(true), openrouter_cloud(true)];
        let config = code_and_research_config();
        let request = RouteRequest {
            task: Some("please fix this rust code".into()),
            ..Default::default()
        };

        let resolved = resolve_route(&providers, &config, &request, false).unwrap();

        assert_eq!(resolved.provider, "ollama");
        assert_eq!(resolved.model, "codellama:7b");
        assert_eq!(resolved.reason, RouteReason::RuleMatch("code-tasks".into()));
    }

    #[test]
    fn research_route() {
        let providers = vec![ollama_local(true), openrouter_cloud(true)];
        let config = code_and_research_config();
        let request = RouteRequest {
            task: Some("please research the latest docs".into()),
            ..Default::default()
        };

        let resolved = resolve_route(&providers, &config, &request, true).unwrap();

        assert_eq!(resolved.provider, "openrouter");
        assert_eq!(resolved.model, "perplexity/sonar-pro");
        assert_eq!(
            resolved.reason,
            RouteReason::RuleMatch("research-tasks".into())
        );
    }

    #[test]
    fn disabled_provider_skipped_for_local_first() {
        let providers = vec![
            ollama_local(false),
            AvailableProvider::new("local-b", true, true, true),
        ];
        let config = RoutingConfig::default();
        let request = RouteRequest::default();

        let resolved = resolve_route(&providers, &config, &request, false).unwrap();

        assert_eq!(resolved.provider, "local-b");
        assert_eq!(resolved.reason, RouteReason::LocalFirst);
    }

    #[test]
    fn overlapping_rules_first_match_wins() {
        let providers = vec![
            ollama_local(true),
            AvailableProvider::new("other", true, true, true),
        ];
        let config = RoutingConfig {
            rules: vec![
                RoutingRule {
                    name: "rule-a".into(),
                    match_contains: vec!["code".into()],
                    provider: "ollama".into(),
                    model: "m1".into(),
                },
                RoutingRule {
                    name: "rule-b".into(),
                    match_contains: vec!["code".into(), "review".into()],
                    provider: "other".into(),
                    model: "m2".into(),
                },
            ],
            fallback: None,
        };
        let request = RouteRequest {
            task: Some("code review".into()),
            ..Default::default()
        };

        let resolved = resolve_route(&providers, &config, &request, false).unwrap();

        assert_eq!(resolved.provider, "ollama");
        assert_eq!(resolved.model, "m1");
        assert_eq!(resolved.reason, RouteReason::RuleMatch("rule-a".into()));
    }

    #[test]
    fn local_first_skips_cloud_even_when_allowed_and_listed_first() {
        let providers = vec![openrouter_cloud(true), ollama_local(true)];
        let config = RoutingConfig::default();
        let request = RouteRequest {
            requested_model: Some("llama3.2:3b".into()),
            ..Default::default()
        };

        // allow_cloud = true, yet the local provider must still win because
        // no rule/fallback selected the cloud provider explicitly.
        let resolved = resolve_route(&providers, &config, &request, true).unwrap();

        assert_eq!(resolved.provider, "ollama");
        assert_eq!(resolved.model, "llama3.2:3b");
        assert_eq!(resolved.reason, RouteReason::LocalFirst);
    }

    #[test]
    fn cloud_forbidden_errors_instead_of_falling_through() {
        let providers = vec![ollama_local(true), openrouter_cloud(true)];
        let config = code_and_research_config();
        let request = RouteRequest {
            task: Some("research this topic".into()),
            ..Default::default()
        };

        let err = resolve_route(&providers, &config, &request, false).unwrap_err();

        assert_eq!(err, RoutingError::CloudNotAllowed("openrouter".into()));
    }

    #[test]
    fn fallback_used_when_no_rule_matches() {
        let providers = vec![openrouter_cloud(true)];
        let config = RoutingConfig {
            rules: vec![],
            fallback: Some(FallbackRoute {
                provider: "openrouter".into(),
                model: "openai/gpt-4.1-mini".into(),
                enabled: true,
            }),
        };
        let request = RouteRequest::default();

        let resolved = resolve_route(&providers, &config, &request, true).unwrap();

        assert_eq!(resolved.provider, "openrouter");
        assert_eq!(resolved.model, "openai/gpt-4.1-mini");
        assert_eq!(resolved.reason, RouteReason::Fallback);
    }

    #[test]
    fn disabled_fallback_falls_through_to_local_first() {
        let providers = vec![openrouter_cloud(true), ollama_local(true)];
        let config = RoutingConfig {
            rules: vec![],
            fallback: Some(FallbackRoute {
                provider: "openrouter".into(),
                model: "openai/gpt-4.1-mini".into(),
                enabled: false,
            }),
        };
        let request = RouteRequest::default();

        let resolved = resolve_route(&providers, &config, &request, true).unwrap();

        assert_eq!(resolved.provider, "ollama");
        assert_eq!(resolved.reason, RouteReason::LocalFirst);
    }

    #[test]
    fn missing_provider_errors() {
        let providers = vec![ollama_local(true)];
        let config = RoutingConfig::default();
        let request = RouteRequest {
            requested_provider: Some("does-not-exist".into()),
            ..Default::default()
        };

        let err = resolve_route(&providers, &config, &request, true).unwrap_err();

        assert_eq!(err, RoutingError::ProviderNotFound("does-not-exist".into()));
    }

    #[test]
    fn model_slug_preserved_exactly() {
        let providers = vec![ollama_local(true)];
        let config = RoutingConfig::default();
        let odd_slug = "Some-Weird.Model:Tag_v2";
        let request = RouteRequest {
            requested_provider: Some("ollama".into()),
            requested_model: Some(odd_slug.into()),
            ..Default::default()
        };

        let resolved = resolve_route(&providers, &config, &request, false).unwrap();

        assert_eq!(resolved.model, odd_slug);
    }

    #[test]
    fn explicit_override_wins_over_matching_rule() {
        let providers = vec![ollama_local(true), openrouter_cloud(true)];
        let config = code_and_research_config();
        let request = RouteRequest {
            requested_provider: Some("ollama".into()),
            requested_model: Some("codellama:7b".into()),
            task: Some("please research this".into()),
        };

        let resolved = resolve_route(&providers, &config, &request, true).unwrap();

        assert_eq!(resolved.provider, "ollama");
        assert_eq!(resolved.model, "codellama:7b");
        assert_eq!(resolved.reason, RouteReason::ExplicitOverride);
    }

    #[test]
    fn explicit_override_missing_key_errors() {
        let providers = vec![AvailableProvider::new("nvidia", true, false, false)];
        let config = RoutingConfig::default();
        let request = RouteRequest {
            requested_provider: Some("nvidia".into()),
            requested_model: Some("some-model".into()),
            ..Default::default()
        };

        let err = resolve_route(&providers, &config, &request, true).unwrap_err();

        assert_eq!(err, RoutingError::ProviderMissingKey("nvidia".into()));
    }

    #[test]
    fn explicit_override_disabled_provider_errors() {
        let providers = vec![ollama_local(false)];
        let config = RoutingConfig::default();
        let request = RouteRequest {
            requested_provider: Some("ollama".into()),
            ..Default::default()
        };

        let err = resolve_route(&providers, &config, &request, false).unwrap_err();

        assert_eq!(err, RoutingError::ProviderDisabled("ollama".into()));
    }

    #[test]
    fn literal_default_model_is_rejected() {
        let providers = vec![ollama_local(true)];
        let config = RoutingConfig::default();
        let request = RouteRequest {
            requested_provider: Some("ollama".into()),
            requested_model: Some("Default".into()),
            ..Default::default()
        };

        let err = resolve_route(&providers, &config, &request, false).unwrap_err();

        assert_eq!(err, RoutingError::InvalidModelSlug("Default".into()));
    }

    #[test]
    fn empty_model_slug_is_not_rejected_and_means_use_provider_default() {
        let providers = vec![ollama_local(true)];
        let config = RoutingConfig::default();
        let request = RouteRequest {
            requested_provider: Some("ollama".into()),
            ..Default::default()
        };

        let resolved = resolve_route(&providers, &config, &request, false).unwrap();

        assert_eq!(resolved.model, "");
    }

    #[test]
    fn no_route_available_errors() {
        let providers: Vec<AvailableProvider> = vec![];
        let config = RoutingConfig::default();
        let request = RouteRequest::default();

        let err = resolve_route(&providers, &config, &request, false).unwrap_err();

        assert_eq!(err, RoutingError::NoRouteAvailable);
    }
}
