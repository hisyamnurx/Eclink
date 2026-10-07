//! Provider profiles: model, validation, and persistence.

pub mod endpoint;
pub mod model;
pub mod store;
pub mod templates;

pub use endpoint::{agent_endpoints, resolve_endpoint, ApiProtocol, ResolveError};
pub use model::{
    is_local_endpoint, is_reserved_header, managed_provider_id, normalize_base_url, slugify,
    user_headers, validate_base_url, AdvancedOptions, AgentId, AuthMode, CompatibilityRecord,
    CompatibilityStatus, HeaderPair, Profile, RESERVED_HEADERS,
};
pub use store::{ProfileStore, ProfilesFile, PROFILES_SCHEMA_VERSION};
pub use templates::{provider_templates, ProviderTemplate};
