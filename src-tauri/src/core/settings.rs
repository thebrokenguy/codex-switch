//! Configurações do app (arquivo portátil ao lado do exe, com fallback).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const MIN_INTERVAL_MINUTES: u32 = 1;
const MAX_INTERVAL_MINUTES: u32 = 120;
const DEFAULT_INTERVAL_MINUTES: u32 = 5;

/// Configurações persistentes do codex-switch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_interval", deserialize_with = "de_interval")]
    pub poll_interval_minutes: u32,
    #[serde(default = "default_true")]
    pub reopen_after_switch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_bin_path: Option<String>,
}

fn default_interval() -> u32 {
    DEFAULT_INTERVAL_MINUTES
}

fn default_true() -> bool {
    true
}

fn de_interval<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let raw = u32::deserialize(d)?;
    Ok(clamp_interval(raw))
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            poll_interval_minutes: DEFAULT_INTERVAL_MINUTES,
            reopen_after_switch: true,
            codex_bin_path: None,
        }
    }
}

/// Limita o intervalo de polling à faixa suportada.
pub fn clamp_interval(minutes: u32) -> u32 {
    minutes.clamp(MIN_INTERVAL_MINUTES, MAX_INTERVAL_MINUTES)
}

/// Carrega as configurações; ausência ou corrupção devolvem os padrões.
pub fn load(path: &Path) -> Settings {
    let Ok(content) = fs::read_to_string(path) else {
        return Settings::default();
    };
    match serde_json::from_str::<Settings>(&content) {
        Ok(mut s) => {
            s.poll_interval_minutes = clamp_interval(s.poll_interval_minutes);
            s
        }
        Err(_) => Settings::default(),
    }
}

/// Grava as configurações com escrita atômica.
pub fn save(path: &Path, settings: &Settings) -> Result<()> {
    let mut settings = settings.clone();
    settings.poll_interval_minutes = clamp_interval(settings.poll_interval_minutes);
    let content = serde_json::to_string_pretty(&settings)?;
    super::auth::write_bytes_atomic(path, content.as_bytes())
        .with_context(|| format!("falha gravando {}", path.display()))?;
    Ok(())
}

/// Caminho padrão do arquivo: ao lado do exe; se não der para escrever lá,
/// cai para a pasta de perfis do Codex.
pub fn resolve_settings_path(exe_dir: Option<PathBuf>, fallback_dir: PathBuf) -> PathBuf {
    let name = "codex-switch.settings.json";
    match exe_dir {
        Some(dir) if dir.is_dir() && can_write(&dir) => dir.join(name),
        _ => fallback_dir.join(name),
    }
}

fn can_write(dir: &Path) -> bool {
    let probe = dir.join(".codex-switch-write-test");
    match fs::write(&probe, b"") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_missing_or_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        assert_eq!(load(&path), Settings::default());

        fs::write(&path, "{broken").unwrap();
        assert_eq!(load(&path), Settings::default());
    }

    #[test]
    fn roundtrip_and_clamping() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");

        let mut s = Settings {
            poll_interval_minutes: 999,
            reopen_after_switch: false,
            codex_bin_path: None,
        };
        save(&path, &s).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.poll_interval_minutes, MAX_INTERVAL_MINUTES);
        assert!(!loaded.reopen_after_switch);

        s.poll_interval_minutes = 0;
        save(&path, &s).unwrap();
        assert_eq!(load(&path).poll_interval_minutes, MIN_INTERVAL_MINUTES);
    }

    #[test]
    fn resolves_next_to_exe_when_writable() {
        let tmp = tempfile::tempdir().unwrap();
        let fallback = tmp.path().join("fallback");
        let exe_dir = tmp.path().join("exe");
        fs::create_dir_all(&exe_dir).unwrap();

        let resolved = resolve_settings_path(Some(exe_dir.clone()), fallback.clone());
        assert_eq!(resolved, exe_dir.join("codex-switch.settings.json"));

        let missing = tmp.path().join("does-not-exist");
        let resolved = resolve_settings_path(Some(missing), fallback.clone());
        assert_eq!(resolved, fallback.join("codex-switch.settings.json"));
    }

    #[test]
    fn preserves_selected_codex_path_in_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(
            &path,
            r#"{
                "poll_interval_minutes": 5,
                "reopen_after_switch": true,
                "codex_bin_path": "C:\\Apps\\Codex\\codex.exe"
            }"#,
        )
        .unwrap();

        let loaded = load(&path);
        let encoded = serde_json::to_value(loaded).unwrap();
        assert_eq!(encoded["codex_bin_path"], "C:\\Apps\\Codex\\codex.exe");
    }
}
