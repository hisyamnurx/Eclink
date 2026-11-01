//! Provider profiles: model, validation, and persistence.

pub mod endpoint;
pub mod model;
pub mod store;
pub mod templates;

pub use endpoint::{
    agent_endpoints, resolve_anthropic_base_base_url, resolve_endpoint, ApiProtocol, ResolveError,
};
pub use model::{
    is_local_endpoint, is_reserved_header, managed_provider_id, normalize_base_url,
    omp_managed_provider_id, slugify, user_headers, validate_base_url, validate_omp_selector,
    AdvancedOptions, AgentId, AuthMode, CompatibilityRecord, CompatibilityStatus, HeaderPair,
    OmpApiProtocol, OmpOptions, Profile, RESERVED_HEADERS,
};
pub use store::{ProfileStore, ProfilesFile, PROFILES_SCHEMA_VERSION};
pub use templates::{provider_templates, ProviderTemplate};
