//! Provider protocol testing (PRD §20, §21).
//!
//! A single "Test Connection" button that only checks reachability would tell
//! the user almost nothing: an endpoint can answer `200 OK` and still be
//! unusable because it does not speak the agent's wire protocol. This module
//! runs the real pipeline the PRD specifies, in order:
//!
//! 1. **Input validation** — reject an impossible profile before any traffic.
//! 2. **Reachability** — can a TCP/TLS connection be established at all?
//! 3. **Authentication** — is the credential accepted?
//! 4. **Model availability** — does the endpoint know the requested model?
//! 5. **Agent protocol** — does it speak the agent's actual API shape?
//! 6. **Streaming** — does SSE arrive in a usable form?
//! 7. **Tool calling** — does a tool round-trip when the endpoint claims it?
//! 8. **Verdict** — a [`CompatibilityStatus`] with an actionable reason.
//!
//! Every step reports a machine `reason_code` plus human copy, so the UI never
//! has to invent an explanation from a bare status number.
//!
//! ## Credential handling
//!
//! The resolved secret is held in a [`secrecy::SecretString`]-like wrapper
//! ([`Secret`]) that deliberately does not implement `Display` or `Serialize`,
//! so it cannot be logged or embedded in a report by accident. Requests are
//! built so the credential only ever appears in a header, and error messages are
//! passed through [`redact_text`] before leaving this module.
//!
//! ## Cost
//!
//! The protocol and tool steps send real (tiny) requests and therefore cost
//! money. The UI shows a cost notice before the first run; this module never
//! hides that fact behind a "ping".

pub mod pipeline;
pub mod playground;
pub mod probes;
pub mod response;
pub mod secret;

pub use pipeline::{test_profile, TestOutcome, TestStep, TestStepState};
pub use response::{parse_for, Framing, ParsedReply};
pub use secret::Secret;
