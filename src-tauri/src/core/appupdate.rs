//! Auto-update do app portátil: checa a Release no GitHub, baixa o exe novo,
//! confere SHA-256 e aplica a troca na próxima abertura.
//!
//! Sem instalador: o updater do Tauri não serve para build portátil, então a
//! troca é feita aqui (renomeia o exe atual, move o novo para o lugar e abre).
//!
//! Fluxo:
//! 1. `check_for_update` lê `update-manifest.json` da release mais recente.
//! 2. Se a versão for maior, baixa o exe da release, confere o SHA-256 e
//!    deixa `codex-switch.update.exe` + `codex-switch.update.json` ao lado.
//! 3. Na próxima abertura, `apply_staged_update` troca os arquivos e reabre
//!    o app já na versão nova.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Manifesto e exe vivem como assets da release mais recente do GitHub.
pub const DEFAULT_UPDATE_BASE: &str =
    "https://github.com/thebrokenguy/codex-switch/releases/latest/download";
pub const MANIFEST_ASSET: &str = "update-manifest.json";
/// Sobrepõe a URL base (usado em testes manuais de ponta a ponta).
pub const UPDATE_BASE_ENV: &str = "CODEX_SWITCH_UPDATE_BASE";

// ---------------------------------------------------------------------------
// Versões
// ---------------------------------------------------------------------------

/// Versão semver simples `vMAJOR.MINOR.PATCH`, com ou sem o prefixo `v`.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let trimmed = text.trim().strip_prefix('v').unwrap_or(text.trim());
    let mut parts = trimmed.split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

pub fn is_newer(remote: (u64, u64, u64), current: (u64, u64, u64)) -> bool {
    remote > current
}

/// Versão remota se for estritamente maior que a atual.
pub fn update_needed(remote_version: &str, current_version: &str) -> Option<(u64, u64, u64)> {
    let remote = parse_version(remote_version)?;
    let current = parse_version(current_version).unwrap_or((0, 0, 0));
    is_newer(remote, current).then_some(remote)
}

// ---------------------------------------------------------------------------
// Manifesto da release e sidecar local
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Manifest {
    pub version: String,
    /// Nome do asset com o exe portátil dentro da release.
    pub asset: String,
    pub sha256: String,
    #[serde(default)]
    pub notes: Option<String>,
}

/// O que fica ao lado do exe baixado, para validar a troca na abertura.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Sidecar {
    pub version: String,
    pub sha256: String,
}

fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn parse_manifest(json: &str) -> Option<Manifest> {
    let manifest: Manifest = serde_json::from_str(json).ok()?;
    if parse_version(&manifest.version).is_none() {
        return None;
    }
    if manifest.asset.trim().is_empty() {
        return None;
    }
    if !is_sha256_hex(&manifest.sha256) {
        return None;
    }
    Some(manifest)
}

fn parse_sidecar(json: &str) -> Option<Sidecar> {
    let sidecar: Sidecar = serde_json::from_str(json).ok()?;
    if parse_version(&sidecar.version).is_none() || !is_sha256_hex(&sidecar.sha256) {
        return None;
    }
    Some(sidecar)
}

// ---------------------------------------------------------------------------
// Hash
// ---------------------------------------------------------------------------

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{:02x}", byte)).collect()
}

pub fn verify_sha256(bytes: &[u8], expected: &str) -> bool {
    sha256_hex(bytes).eq_ignore_ascii_case(expected)
}

// ---------------------------------------------------------------------------
// Troca do exe (staging)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct StagedPaths {
    pub staged: PathBuf,
    pub sidecar: PathBuf,
    pub old: PathBuf,
}

pub fn staging_paths(exe: &Path) -> StagedPaths {
    let dir = exe.parent().unwrap_or_else(|| Path::new("."));
    let name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "codex-switch.exe".to_string());
    StagedPaths {
        staged: dir.join("codex-switch.update.exe"),
        sidecar: dir.join("codex-switch.update.json"),
        old: dir.join(format!("{name}.old")),
    }
}

/// Aplica o exe staged: valida o hash do sidecar, guarda o exe atual como
/// `.old` e move o novo para o lugar. Em falha no meio, devolve o original.
pub fn stage_swap(exe: &Path) -> Result<(), String> {
    let paths = staging_paths(exe);
    if !paths.staged.exists() {
        return Err("nenhuma atualização staged".to_string());
    }
    let sidecar_raw = std::fs::read_to_string(&paths.sidecar)
        .map_err(|e| format!("lendo sidecar da atualização: {e}"))?;
    let sidecar = parse_sidecar(&sidecar_raw).ok_or("sidecar da atualização inválido")?;
    let bytes =
        std::fs::read(&paths.staged).map_err(|e| format!("lendo exe staged: {e}"))?;
    if !verify_sha256(&bytes, &sidecar.sha256) {
        return Err("hash do exe staged não confere com o manifesto".to_string());
    }

    let _ = std::fs::remove_file(&paths.old);
    std::fs::rename(exe, &paths.old).map_err(|e| format!("guardando exe atual: {e}"))?;
    if let Err(e) = std::fs::rename(&paths.staged, exe) {
        return match std::fs::rename(&paths.old, exe) {
            Ok(()) => Err(format!("movendo exe novo para o lugar: {e}")),
            Err(rollback) => Err(format!(
                "movendo exe novo para o lugar: {e}; rollback do original também falhou: {rollback}"
            )),
        };
    }
    Ok(())
}

fn cleanup_old(exe: &Path) {
    let paths = staging_paths(exe);
    let _ = std::fs::remove_file(&paths.old);
    if !paths.staged.exists() {
        let _ = std::fs::remove_file(&paths.sidecar);
    }
}

/// Chamado no início do app: se há atualização staged e válida, troca o exe,
/// abre a versão nova e encerra esta instância.
pub fn apply_staged_update() {
    if cfg!(debug_assertions) {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let paths = staging_paths(&exe);
    if !paths.staged.exists() {
        cleanup_old(&exe);
        return;
    }
    if let Err(e) = stage_swap(&exe) {
        eprintln!("aviso: falha aplicando atualização: {e}");
        return;
    }
    match std::process::Command::new(&exe)
        .args(std::env::args().skip(1))
        .spawn()
    {
        Ok(_) => std::process::exit(0),
        Err(e) => eprintln!("aviso: falha abrindo a versão nova: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Checagem e download
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpdateStatus {
    /// "skipped" | "up_to_date" | "staged" | "error"
    pub status: String,
    pub version: Option<String>,
    pub message: Option<String>,
}

impl UpdateStatus {
    fn new(status: &str) -> Self {
        Self {
            status: status.to_string(),
            version: None,
            message: None,
        }
    }

    fn with_version(mut self, version: &str) -> Self {
        self.version = Some(version.to_string());
        self
    }

    fn with_message(mut self, message: &str) -> Self {
        self.message = Some(message.to_string());
        self
    }
}

/// Baixa o manifesto da release mais recente e, se houver versão maior,
/// baixa o exe, confere o SHA-256 e deixa staged para a próxima abertura.
pub async fn check_and_stage(
    client: &reqwest::Client,
    base: &str,
    current_version: &str,
    exe: &Path,
) -> UpdateStatus {
    let manifest_url = format!("{}/{}", base.trim_end_matches('/'), MANIFEST_ASSET);
    let manifest_raw = match client.get(&manifest_url).send().await {
        Ok(response) => match response.error_for_status() {
            Ok(response) => match response.text().await {
                Ok(text) => text,
                Err(e) => return UpdateStatus::new("error").with_message(&format!("lendo manifesto: {e}")),
            },
            Err(e) => return UpdateStatus::new("error").with_message(&format!("buscando manifesto: {e}")),
        },
        Err(e) => return UpdateStatus::new("error").with_message(&format!("buscando manifesto: {e}")),
    };
    let manifest = match parse_manifest(&manifest_raw) {
        Some(manifest) => manifest,
        None => return UpdateStatus::new("error").with_message("manifesto da release inválido"),
    };
    if update_needed(&manifest.version, current_version).is_none() {
        return UpdateStatus::new("up_to_date");
    }

    let exe_url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        manifest.asset
    );
    let bytes = match client.get(&exe_url).send().await {
        Ok(response) => match response.error_for_status() {
            Ok(response) => match response.bytes().await {
                Ok(bytes) => bytes,
                Err(e) => return UpdateStatus::new("error").with_message(&format!("baixando exe: {e}")),
            },
            Err(e) => return UpdateStatus::new("error").with_message(&format!("baixando exe: {e}")),
        },
        Err(e) => return UpdateStatus::new("error").with_message(&format!("baixando exe: {e}")),
    };
    if !verify_sha256(&bytes, &manifest.sha256) {
        return UpdateStatus::new("error").with_message("hash do exe baixado não confere com o manifesto");
    }

    let paths = staging_paths(exe);
    let sidecar = Sidecar {
        version: manifest.version.clone(),
        sha256: manifest.sha256.clone(),
    };
    let sidecar_raw = match serde_json::to_string(&sidecar) {
        Ok(text) => text,
        Err(e) => return UpdateStatus::new("error").with_message(&format!("serializando sidecar: {e}")),
    };
    if let Err(e) = std::fs::write(&paths.staged, &bytes) {
        return UpdateStatus::new("error").with_message(&format!("gravando exe staged: {e}"));
    }
    if let Err(e) = std::fs::write(&paths.sidecar, sidecar_raw) {
        let _ = std::fs::remove_file(&paths.staged);
        return UpdateStatus::new("error").with_message(&format!("gravando sidecar: {e}"));
    }
    UpdateStatus::new("staged").with_version(&manifest.version)
}

#[tauri::command]
pub async fn check_for_update() -> UpdateStatus {
    if cfg!(debug_assertions) {
        return UpdateStatus::new("skipped").with_message("modo dev não atualiza");
    }
    let Ok(exe) = std::env::current_exe() else {
        return UpdateStatus::new("error").with_message("caminho do exe indisponível");
    };
    let base = std::env::var(UPDATE_BASE_ENV).unwrap_or_else(|_| DEFAULT_UPDATE_BASE.to_string());
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
    {
        Ok(client) => client,
        Err(e) => return UpdateStatus::new("error").with_message(&format!("criando cliente HTTP: {e}")),
    };
    check_and_stage(&client, &base, env!("CARGO_PKG_VERSION"), &exe).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Versões
    // -----------------------------------------------------------------------

    #[test]
    fn parse_version_accepts_v_prefix_and_plain() {
        assert_eq!(parse_version("v0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_version("0.2.1"), Some((0, 2, 1)));
        assert_eq!(parse_version("1.10.0"), Some((1, 10, 0)));
    }

    #[test]
    fn parse_version_rejects_malformed() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("abc"), None);
        assert_eq!(parse_version("1.2.3-beta"), None);
        assert_eq!(parse_version("1.2.x"), None);
    }

    #[test]
    fn is_newer_compares_numerically() {
        assert!(is_newer((0, 2, 1), (0, 2, 0)));
        assert!(is_newer((1, 0, 0), (0, 9, 9)));
        assert!(!is_newer((0, 2, 0), (0, 2, 0)));
        assert!(!is_newer((0, 1, 9), (0, 2, 0)));
        assert!(is_newer((0, 2, 10), (0, 2, 9)));
    }

    #[test]
    fn update_needed_only_for_newer_remote() {
        assert_eq!(update_needed("v0.2.1", "0.2.0"), Some((0, 2, 1)));
        assert_eq!(update_needed("v0.2.0", "0.2.0"), None);
        assert_eq!(update_needed("v0.1.9", "0.2.0"), None);
        assert_eq!(update_needed("inválida", "0.2.0"), None);
    }

    // -----------------------------------------------------------------------
    // Manifesto da release
    // -----------------------------------------------------------------------

    #[test]
    fn parse_manifest_reads_required_fields() {
        let json = r#"{
            "version": "0.2.1",
            "asset": "codex-switch-portable.exe",
            "sha256": "d9779c92eb4b2f46fcd59fc1ae0202a8158383b7ff6ab2ed2b5b8285d061a7d2",
            "notes": "correções"
        }"#;
        let manifest = parse_manifest(json).expect("manifesto válido");
        assert_eq!(manifest.version, "0.2.1");
        assert_eq!(manifest.asset, "codex-switch-portable.exe");
        assert_eq!(manifest.notes.as_deref(), Some("correções"));
    }

    #[test]
    fn parse_manifest_rejects_missing_or_invalid_fields() {
        let sem_sha = r#"{"version":"0.2.1","asset":"codex-switch-portable.exe"}"#;
        assert!(parse_manifest(sem_sha).is_none());

        let sha_curto = r#"{"version":"0.2.1","asset":"a.exe","sha256":"abc"}"#;
        assert!(parse_manifest(sha_curto).is_none());

        let sem_asset = r#"{"version":"0.2.1","sha256":"d9779c92eb4b2f46fcd59fc1ae0202a8158383b7ff6ab2ed2b5b8285d061a7d2"}"#;
        assert!(parse_manifest(sem_asset).is_none());

        assert!(parse_manifest("não é json").is_none());
    }

    // -----------------------------------------------------------------------
    // Hash
    // -----------------------------------------------------------------------

    #[test]
    fn sha256_hex_matches_known_value() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert!(verify_sha256(b"abc", &sha256_hex(b"abc")));
        assert!(!verify_sha256(b"abc", &sha256_hex(b"abd")));
    }

    // -----------------------------------------------------------------------
    // Troca do exe (staging)
    // -----------------------------------------------------------------------

    #[test]
    fn staging_paths_are_next_to_the_exe() {
        let exe = std::path::Path::new("C:/apps/codex-switch/codex-switch.exe");
        let paths = staging_paths(exe);
        assert_eq!(paths.staged.file_name().unwrap(), "codex-switch.update.exe");
        assert_eq!(
            paths.sidecar.file_name().unwrap(),
            "codex-switch.update.json"
        );
        assert_eq!(paths.old.file_name().unwrap(), "codex-switch.exe.old");
    }

    #[test]
    fn stage_swap_replaces_exe_and_keeps_backup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("codex-switch.exe");
        std::fs::write(&exe, b"ANTIGO").unwrap();
        std::fs::write(dir.path().join("codex-switch.update.exe"), b"NOVO").unwrap();
        let sidecar = format!(
            r#"{{"version":"0.2.1","sha256":"{}"}}"#,
            sha256_hex(b"NOVO")
        );
        std::fs::write(dir.path().join("codex-switch.update.json"), sidecar).unwrap();

        stage_swap(&exe).expect("troca deve funcionar");

        assert_eq!(std::fs::read(&exe).unwrap(), b"NOVO");
        assert_eq!(
            std::fs::read(dir.path().join("codex-switch.exe.old")).unwrap(),
            b"ANTIGO"
        );
    }

    #[test]
    fn stage_swap_rejects_hash_mismatch_without_touching_exe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("codex-switch.exe");
        std::fs::write(&exe, b"ANTIGO").unwrap();
        std::fs::write(dir.path().join("codex-switch.update.exe"), b"NOVO").unwrap();
        let sidecar = format!(
            r#"{{"version":"0.2.1","sha256":"{}"}}"#,
            sha256_hex(b"OUTRO")
        );
        std::fs::write(dir.path().join("codex-switch.update.json"), sidecar).unwrap();

        assert!(stage_swap(&exe).is_err());

        assert_eq!(std::fs::read(&exe).unwrap(), b"ANTIGO");
        assert!(dir.path().join("codex-switch.update.exe").exists());
    }

    #[test]
    fn stage_swap_without_staging_is_a_no_op() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("codex-switch.exe");
        std::fs::write(&exe, b"ANTIGO").unwrap();

        assert!(stage_swap(&exe).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"ANTIGO");
    }
}
