//! Login isolado: autentica uma conta nova num CODEX_HOME temporário,
//! sem mexer no login atual (fluxo `login-as`).

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::SystemTime;

const LOGIN_HOME_PREFIX: &str = "codex-switch-login-";
const STALE_LOGIN_HOME_AFTER_MILLIS: i64 = 60 * 60 * 1000;

/// Cria um diretório temporário que servirá de CODEX_HOME para o login isolado.
/// Inclui pid + contador e usa `create_dir` (atômico) para nunca repetir caminho.
pub fn prepare_isolated_home() -> Result<PathBuf> {
    use anyhow::bail;
    use std::io::ErrorKind;
    let base = std::env::temp_dir();
    cleanup_stale_isolated_homes();
    let stamp = chrono::Utc::now().timestamp_millis();
    let pid = std::process::id();
    for attempt in 0..1000u32 {
        let dir = base.join(format!("{LOGIN_HOME_PREFIX}{stamp}-{pid}-{attempt}"));
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("falha criando {}", dir.display())),
        }
    }
    bail!(
        "não consegui criar pasta temporária única em {}",
        base.display()
    );
}

/// Remove diretórios de logins isolados deixados por uma execução interrompida.
/// Só considera entradas com timestamp no nome e mais antigas que uma hora.
pub fn cleanup_stale_isolated_homes() {
    let base = std::env::temp_dir();
    let now = chrono::Utc::now().timestamp_millis();
    let Ok(entries) = fs::read_dir(&base) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(stamp) = name
            .strip_prefix(LOGIN_HOME_PREFIX)
            .and_then(|rest| rest.split_once('-'))
            .and_then(|(stamp, _)| stamp.parse::<i64>().ok())
        else {
            continue;
        };
        if now.saturating_sub(stamp) > STALE_LOGIN_HOME_AFTER_MILLIS {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[cfg(windows)]
fn hide_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    // CREATE_NO_WINDOW: evita piscar um console ao abrir o processo.
    cmd.creation_flags(0x0800_0000);
}

/// Sobe o processo `codex login` com `CODEX_HOME` apontando para a pasta isolada.
pub fn spawn_login(codex_exe: &Path, home: &Path) -> Result<Child> {
    let mut cmd = Command::new(codex_exe);
    cmd.arg("login")
        .env("CODEX_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    hide_window(&mut cmd);
    cmd.spawn()
        .with_context(|| format!("não consegui iniciar {}", codex_exe.display()))
}

/// Caminho do auth.json dentro da home isolada.
pub fn isolated_auth_path(home: &Path) -> PathBuf {
    home.join("auth.json")
}

/// Apaga a pasta temporária do login isolado.
pub fn cleanup(home: &Path) {
    let _ = fs::remove_dir_all(home);
}

fn is_pe_executable(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let mut dos_header = [0u8; 64];
    if file.read_exact(&mut dos_header).is_err() || &dos_header[..2] != b"MZ" {
        return false;
    }
    let pe_offset = u32::from_le_bytes([
        dos_header[0x3c],
        dos_header[0x3d],
        dos_header[0x3e],
        dos_header[0x3f],
    ]) as u64;
    if pe_offset > 16 * 1024 * 1024 || file.seek(SeekFrom::Start(pe_offset)).is_err() {
        return false;
    }
    let mut signature = [0u8; 4];
    file.read_exact(&mut signature).is_ok() && &signature == b"PE\0\0"
}

/// Valida o arquivo escolhido manualmente na interface.
pub fn validate_codex_bin(raw: &str) -> Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("selecione o arquivo codex.exe");
    }
    let candidate = PathBuf::from(trimmed);
    if !candidate.is_file() {
        bail!("não encontrei o arquivo escolhido: {}", candidate.display());
    }
    let file_name = candidate
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !file_name.eq_ignore_ascii_case("codex.exe") {
        bail!("selecione o arquivo codex.exe");
    }
    if !is_pe_executable(&candidate) {
        bail!("o arquivo escolhido não parece ser um executável Windows válido");
    }
    Ok(candidate)
}

fn is_valid_executable_candidate(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("codex.exe"))
        && is_pe_executable(path)
}

fn is_valid_path_candidate(path: &Path) -> bool {
    if is_valid_executable_candidate(path) {
        return true;
    }
    path.is_file()
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("codex.cmd") || name.eq_ignore_ascii_case("codex.bat")
            })
}

/// Procura o executável do Codex CLI.
///
/// Ordem: variável `CODEX_SWITCH_CODEX_BIN`, caminho escolhido na interface,
/// instalação separada em `%LOCALAPPDATA%\Programs\OpenAI\Codex\bin\codex.exe`,
/// binários do app desktop em `%LOCALAPPDATA%\OpenAI\Codex\bin\`, depois PATH.
pub fn find_codex_cli(configured: Option<&Path>) -> Option<PathBuf> {
    if let Ok(custom) = std::env::var("CODEX_SWITCH_CODEX_BIN") {
        let p = PathBuf::from(custom);
        if is_valid_executable_candidate(&p) {
            return Some(p);
        }
    }
    if let Some(p) = configured {
        if validate_codex_bin(&p.to_string_lossy()).is_ok() {
            return Some(p.to_path_buf());
        }
    }
    if let Some(local) = dirs::data_local_dir() {
        let separate = local
            .join("Programs")
            .join("OpenAI")
            .join("Codex")
            .join("bin")
            .join("codex.exe");
        if is_valid_executable_candidate(&separate) {
            return Some(separate);
        }
        if let Some(managed) = find_managed_codex(&local) {
            return Some(managed);
        }
    }
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in ["codex.exe", "codex.cmd", "codex.bat"] {
            let candidate = dir.join(name);
            if is_valid_path_candidate(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// Executável do Codex embutido no app desktop, em
/// `%LOCALAPPDATA%\OpenAI\Codex\bin\`: aceita o `codex.exe` direto ou o de
/// subpastas com hash de versão. Entre os candidatos, vence o mais recente.
fn find_managed_codex(data_local: &Path) -> Option<PathBuf> {
    let bin = data_local.join("OpenAI").join("Codex").join("bin");
    let mut candidates = vec![bin.join("codex.exe")];
    if let Ok(entries) = fs::read_dir(&bin) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                candidates.push(entry.path().join("codex.exe"));
            }
        }
    }
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for candidate in candidates {
        let meta = match fs::metadata(&candidate) {
            Ok(m) if m.is_file() && is_valid_executable_candidate(&candidate) => m,
            _ => continue,
        };
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(t, _)| modified > *t) {
            best = Some((modified, candidate));
        }
    }
    best.map(|(_, path)| path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_creates_unique_directory() {
        let a = prepare_isolated_home().unwrap();
        let b = prepare_isolated_home().unwrap();
        assert!(a.is_dir());
        assert!(b.is_dir());
        assert_ne!(a, b);
        cleanup(&a);
        cleanup(&b);
        assert!(!a.exists());
    }

    #[test]
    fn prepare_cleans_stale_orphan_homes() {
        let stale =
            std::env::temp_dir().join(format!("codex-switch-login-0-{}-0", std::process::id()));
        cleanup(&stale);
        fs::create_dir_all(&stale).unwrap();

        let fresh = prepare_isolated_home().unwrap();

        assert!(!stale.exists());
        cleanup(&fresh);
    }

    #[test]
    fn isolated_auth_path_points_inside_home() {
        let home = PathBuf::from("C:/tmp/example");
        assert_eq!(isolated_auth_path(&home), home.join("auth.json"));
    }

    fn make_bin_layout(root: &Path) -> PathBuf {
        let bin = root.join("OpenAI").join("Codex").join("bin");
        fs::create_dir_all(&bin).unwrap();
        bin
    }

    fn write_exe(path: &Path, secs: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_fake_pe(path);
        let file = fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }

    fn write_fake_pe(path: &Path) {
        let mut bytes = vec![0u8; 128];
        bytes[0..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&(64u32).to_le_bytes());
        bytes[64..68].copy_from_slice(b"PE\0\0");
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn validate_codex_bin_accepts_existing_codex_exe() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("codex.exe");
        write_fake_pe(&path);

        assert_eq!(validate_codex_bin(path.to_str().unwrap()).unwrap(), path);
    }

    #[test]
    fn validate_codex_bin_rejects_non_executable_payload() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("codex.exe");
        fs::write(&path, b"not a Windows executable").unwrap();

        assert!(validate_codex_bin(path.to_str().unwrap()).is_err());
    }

    #[test]
    fn validate_codex_bin_rejects_missing_or_wrong_file() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("codex.exe");
        assert!(validate_codex_bin(missing.to_str().unwrap()).is_err());

        let wrong = tmp.path().join("other.exe");
        fs::write(&wrong, b"fake").unwrap();
        assert!(validate_codex_bin(wrong.to_str().unwrap()).is_err());
    }

    #[test]
    fn find_codex_cli_prefers_valid_configured_path() {
        let tmp = tempfile::tempdir().unwrap();
        let selected = tmp.path().join("codex.exe");
        write_fake_pe(&selected);

        assert_eq!(
            find_codex_cli(Some(&selected)),
            Some(selected),
            "o caminho escolhido deve ser aceito quando existe",
        );
    }

    #[test]
    fn find_codex_cli_rejects_replaced_configured_file() {
        let tmp = tempfile::tempdir().unwrap();
        let selected = tmp.path().join("codex.exe");
        fs::write(&selected, b"not a Windows executable").unwrap();

        assert!(!is_valid_executable_candidate(&selected));
        assert!(validate_codex_bin(selected.to_str().unwrap()).is_err());
    }

    #[test]
    fn find_managed_codex_returns_none_without_layout() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(find_managed_codex(tmp.path()).is_none());
    }

    #[test]
    fn find_managed_codex_picks_newest_hash_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = make_bin_layout(tmp.path());
        write_exe(&bin.join("aaaa").join("codex.exe"), 1_000);
        write_exe(&bin.join("bbbb").join("codex.exe"), 2_000);
        fs::create_dir_all(bin.join("cccc")).unwrap();
        let picked = find_managed_codex(tmp.path()).unwrap();
        assert_eq!(picked, bin.join("bbbb").join("codex.exe"));
    }

    #[test]
    fn find_managed_codex_prefers_most_recent_between_direct_and_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = make_bin_layout(tmp.path());
        write_exe(&bin.join("codex.exe"), 3_000);
        write_exe(&bin.join("aaaa").join("codex.exe"), 2_000);
        let picked = find_managed_codex(tmp.path()).unwrap();
        assert_eq!(picked, bin.join("codex.exe"));

        write_exe(&bin.join("aaaa").join("codex.exe"), 4_000);
        let picked = find_managed_codex(tmp.path()).unwrap();
        assert_eq!(picked, bin.join("aaaa").join("codex.exe"));
    }
}
