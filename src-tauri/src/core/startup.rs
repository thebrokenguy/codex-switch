//! Inicialização automática com o Windows, via atalho na pasta Inicializar
//! (sem tocar no registro).
//!
//! O estado é puramente do sistema de arquivos: se o atalho existe, está ligado.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

/// Nome do atalho criado na pasta Inicializar.
pub const SHORTCUT_NAME: &str = "codex-switch.lnk";

/// Pasta Inicializar do usuário atual.
pub fn startup_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|c| {
        c.join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
            .join("Startup")
    })
}

/// Caminho do atalho dentro de uma pasta Inicializar (dado explícito para testes).
pub fn shortcut_path_in(dir: &Path) -> PathBuf {
    dir.join(SHORTCUT_NAME)
}

/// `true` se o atalho existe nessa pasta.
pub fn is_enabled_in(dir: &Path) -> bool {
    shortcut_path_in(dir).is_file()
}

/// Liga/desliga usando a pasta Inicializar e o executável atuais.
pub fn set_enabled(enabled: bool) -> Result<()> {
    let dir = startup_dir().context("pasta Inicializar não encontrada")?;
    let exe = std::env::current_exe().context("caminho do executável atual")?;
    set_enabled_in(&dir, &exe, enabled)
}

/// Estado atual relativo ao atalho real.
pub fn is_enabled() -> bool {
    startup_dir().map(|d| is_enabled_in(&d)).unwrap_or(false)
}

/// Liga/desliga o atalho numa pasta qualquer (testável).
///
/// Ligar cria um `.lnk` apontando para `exe` via `WScript.Shell` (PowerShell
/// oculto). Desligar remove o atalho; ausência já é o estado desejado.
pub fn set_enabled_in(dir: &Path, exe: &Path, enabled: bool) -> Result<()> {
    let lnk = shortcut_path_in(dir);
    if enabled {
        fs::create_dir_all(dir).with_context(|| format!("criando {}", dir.display()))?;
        let workdir = exe.parent().unwrap_or_else(|| Path::new(""));
        let script = format!(
            "$ws = New-Object -ComObject WScript.Shell; \
             $s = $ws.CreateShortcut({lnk}); \
             $s.TargetPath = {exe}; \
             $s.WorkingDirectory = {workdir}; \
             $s.Save()",
            lnk = ps_quote(&lnk.display().to_string()),
            exe = ps_quote(&exe.display().to_string()),
            workdir = ps_quote(&workdir.display().to_string()),
        );
        let output = super::codex_process::hidden_command("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .context("falha executando PowerShell para criar o atalho")?;
        if !output.status.success() {
            bail!(
                "PowerShell falhou criando o atalho: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        if !lnk.is_file() {
            bail!("atalho não apareceu em {}", lnk.display());
        }
        Ok(())
    } else {
        match fs::remove_file(&lnk) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removendo {}", lnk.display())),
        }
    }
}

/// Escapa uma string para uso dentro de aspas simples do PowerShell.
fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe_existente() -> PathBuf {
        PathBuf::from(r"C:\Windows\System32\notepad.exe")
    }

    #[test]
    fn enable_creates_shortcut_and_reports_enabled() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            !is_enabled_in(dir.path()),
            "estado inicial deve ser desligado"
        );

        set_enabled_in(dir.path(), &exe_existente(), true).unwrap();

        assert!(
            is_enabled_in(dir.path()),
            "atalho deveria existir após ligar"
        );
        let lnk = shortcut_path_in(dir.path());
        assert_eq!(lnk.file_name().unwrap(), "codex-switch.lnk");
        let meta = fs::metadata(&lnk).unwrap();
        assert!(meta.len() > 0, "atalho não pode ser vazio");
    }

    #[test]
    fn disable_removes_shortcut_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();

        // Desligar quando nem existe: ok.
        set_enabled_in(dir.path(), &exe_existente(), false).unwrap();
        assert!(!is_enabled_in(dir.path()));

        set_enabled_in(dir.path(), &exe_existente(), true).unwrap();
        assert!(is_enabled_in(dir.path()));

        set_enabled_in(dir.path(), &exe_existente(), false).unwrap();
        assert!(
            !is_enabled_in(dir.path()),
            "atalho deveria sumir após desligar"
        );
    }
}
