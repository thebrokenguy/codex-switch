//! Núcleo do codex-switch: lógica sem dependência de UI.
//!
//! Módulos:
//! - `paths`: resolução de caminhos (home do Codex, pasta de perfis, backups).
//! - `auth`: leitura/escrita atômica do auth.json e claims do id_token.

pub mod auth;
pub mod codex_process;
pub mod format;
pub mod login;
pub mod paths;
pub mod profiles;
pub mod settings;
pub mod startup;
pub mod usage;
