//! Config mutation engine.
//!
//! Everything that writes agent configuration goes through this module so the
//! safety sequence (read → parse → validate → hash → snapshot → patch → temp →
//! reparse → atomic replace → verify) is impossible to skip by accident
//! (PRD §31, §47).

pub mod fs;
pub mod json_patch;
pub mod toml_patch;

pub use fs::{
    apply_newline, decode_text, detect_newline, read_to_string_opt, sha256_file, sha256_hex,
    write_atomic, FileLock,
};
