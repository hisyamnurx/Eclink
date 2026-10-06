//! Provider profiles: model, validation, and persistence.

pub mod model;
pub mod store;
pub mod templates;

pub use model::{
    is_local_endpoint, managed_provider_id, normalize_base_url, slugify, validate_base_url,
    AdvancedOptions, AgentId, AuthMode, CompatibilityRecord, CompatibilityStatus, HeaderPair,
    Profile,
};
pub use store::{ProfileStore, ProfilesFile, PROFILES_SCHEMA_VERSION};
pub use templates::{provider_templates, ProviderTemplate};
