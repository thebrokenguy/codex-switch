//! Detecção e controle dos processos do Codex (app desktop e CLI).

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

/// Informação mínima de um processo.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProcInfo {
    #[serde(rename = "ProcessId")]
    pub pid: u32,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "ExecutablePath", default)]
    pub path: Option<String>,
}

/// Faz o parse da saída JSON do PowerShell (array ou objeto único).
pub fn parse_process_json(s: &str) -> Vec<ProcInfo> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(serde_json::Value::Array(items)) => items
            .into_iter()
            .filter_map(|i| serde_json::from_value(i).ok())
            .collect(),
        Ok(obj @ serde_json::Value::Object(_)) => {
            serde_json::from_value(obj).ok().into_iter().collect()
        }
        _ => Vec::new(),
    }
}

/// É um processo do Codex que deve ser considerado? (app desktop, CLI, auxiliares)
pub fn is_codex_process(p: &ProcInfo) -> bool {
    let name = p.name.to_ascii_lowercase();
    if name == "codex-switch.exe" {
        return false;
    }
    let path = p.path.as_deref().unwrap_or("").to_ascii_lowercase();
    if path.contains("codex-switch") {
        return false;
    }
    if name == "codex-windows-sandbox-service.exe" {
        // Serviço do Windows: não mexe.
        return false;
    }
    if name == "openai.codex.exe" {
        return false;
    }
    if name == "chatgpt.exe" {
        // Só o app do pacote Codex (não um ChatGPT avulso, se existir).
        return path.contains("openai.codex");
    }
    name == "codex.exe" || name.starts_with("codex-")
}

/// É o processo principal do app desktop do Codex?
pub fn is_desktop_app(p: &ProcInfo) -> bool {
    p.name.eq_ignore_ascii_case("chatgpt.exe")
        && p.path
            .as_deref()
            .map(|p| p.to_ascii_lowercase().contains("openai.codex"))
            .unwrap_or(false)
}

pub(crate) fn hidden_command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

/// Lista os processos do Codex em execução (via PowerShell/CIM).
pub fn list_codex_processes() -> Result<Vec<ProcInfo>> {
    let script = "Get-CimInstance Win32_Process | \
        Where-Object { $_.Name -like 'codex*' -or $_.Name -eq 'ChatGPT.exe' } | \
        Select-Object ProcessId,Name,ExecutablePath | ConvertTo-Json -Compress";
    let output = hidden_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .context("falha consultando processos")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_process_json(&stdout)
        .into_iter()
        .filter(is_codex_process)
        .collect())
}

/// Fecha um processo pelo PID (taskkill /F).
pub fn kill_process(pid: u32) -> Result<()> {
    let output = hidden_command("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output()
        .context("falha executando taskkill")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("taskkill {pid} falhou: {}", stderr.trim());
    }
    Ok(())
}

/// Fecha todos os processos do Codex indicados. Retorna quantos fecharam.
pub fn close_codex_processes(procs: &[ProcInfo]) -> (usize, Vec<String>) {
    let mut closed = 0usize;
    let mut errors = Vec::new();
    for p in procs {
        match kill_process(p.pid) {
            Ok(()) => closed += 1,
            Err(err) => errors.push(format!("{} (pid {}): {err}", p.name, p.pid)),
        }
    }
    (closed, errors)
}

/// Reabre o app desktop do Codex via `codex app` (destacado, sem console).
pub fn reopen_codex_app(codex_exe: &Path) -> Result<()> {
    let mut cmd = Command::new(codex_exe);
    cmd.arg("app")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW | DETACHED_PROCESS
        cmd.creation_flags(0x0800_0000 | 0x0000_0008);
    }
    cmd.spawn()
        .with_context(|| format!("não consegui abrir {}", codex_exe.display()))?;
    Ok(())
}

/// Monta o alvo do explorer (`shell:AppsFolder\...`) para abrir um app pelo AppID.
pub fn apps_folder_target(app_id: &str) -> String {
    format!(r"shell:AppsFolder\{app_id}")
}

/// Descobre o AppID do app desktop do Codex (via Get-StartApps). None se não achar.
pub fn discover_desktop_app_id() -> Option<String> {
    let script = "Get-StartApps | Where-Object { $_.AppID -like '*Codex*' } | \
        Select-Object -First 1 -ExpandProperty AppID";
    let output = hidden_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_first_nonempty_line(&stdout)
}

/// Abre o app desktop do Codex pelo AppID (mesmo efeito de clicar no menu Iniciar).
pub fn open_desktop_app_via_appid(app_id: &str) -> Result<()> {
    let mut cmd = Command::new("explorer.exe");
    cmd.arg(apps_folder_target(app_id));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.spawn()
        .context("não consegui abrir o app do Codex pelo explorer")?;
    Ok(())
}

/// Reabre o app desktop do Codex: tenta pelo AppID; sem ele, cai para `codex app`.
/// Devolve true quando conseguiu disparar a abertura.
pub fn reopen_desktop_app(codex_exe: Option<&Path>) -> Result<bool> {
    if let Some(app_id) = discover_desktop_app_id() {
        if open_desktop_app_via_appid(&app_id).is_ok() {
            return Ok(true);
        }
    }
    if let Some(exe) = codex_exe {
        reopen_codex_app(exe)?;
        return Ok(true);
    }
    Ok(false)
}

/// Primeira linha não vazia de um texto (usado na descoberta do AppID).
pub fn parse_first_nonempty_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_first_nonempty_line_trims_and_skips_blanks() {
        assert_eq!(
            parse_first_nonempty_line("\r\n  OpenAI.Codex_abc!App  \r\n").as_deref(),
            Some("OpenAI.Codex_abc!App")
        );
        assert_eq!(parse_first_nonempty_line("   \r\n"), None);
    }

    #[test]
    fn apps_folder_target_wraps_appid() {
        assert_eq!(
            apps_folder_target("OpenAI.Codex_abc!App"),
            r"shell:AppsFolder\OpenAI.Codex_abc!App"
        );
    }

    fn proc(name: &str, path: &str) -> ProcInfo {
        ProcInfo {
            pid: 1,
            name: name.to_string(),
            path: Some(path.to_string()),
        }
    }

    #[test]
    fn detects_codex_and_desktop_processes() {
        assert!(is_codex_process(&proc(
            "codex.exe",
            r"C:\Users\x\AppData\Local\Programs\OpenAI\Codex\bin\codex.exe"
        )));
        assert!(is_codex_process(&proc(
            "ChatGPT.exe",
            r"C:\Program Files\WindowsApps\OpenAI.Codex_26.915.3509.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe"
        )));
        assert!(is_codex_process(&proc(
            "codex-computer-use-swift.exe",
            r"C:\Users\x\AppData\Local\OpenAI\Codex\runtimes\cua_node\bin\codex-computer-use-swift.exe"
        )));
    }

    #[test]
    fn ignores_service_switch_app_and_foreign_chatgpt() {
        assert!(!is_codex_process(&proc(
            "codex-windows-sandbox-service.exe",
            r"C:\Windows\System32\codex-windows-sandbox-service.exe"
        )));
        assert!(!is_codex_process(&proc(
            "codex-switch.exe",
            r"C:\workspace\codex-switch\target\release\codex-switch.exe"
        )));
        assert!(!is_codex_process(&proc(
            "ChatGPT.exe",
            r"C:\Program Files\WindowsApps\OpenAI.ChatGPT_1.0\app\ChatGPT.exe"
        )));
        assert!(!is_codex_process(&proc(
            "notepad.exe",
            r"C:\Windows\notepad.exe"
        )));
    }

    #[test]
    fn desktop_app_detection_requires_openai_codex_path() {
        let desktop = proc(
            "ChatGPT.exe",
            r"C:\Program Files\WindowsApps\OpenAI.Codex_26.915.3509.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe",
        );
        assert!(is_desktop_app(&desktop));
        let cli = proc(
            "codex.exe",
            r"C:\Users\x\AppData\Local\Programs\OpenAI\Codex\bin\codex.exe",
        );
        assert!(!is_desktop_app(&cli));
    }

    #[test]
    fn parses_powershell_json_array_and_single_object() {
        let arr = r#"[{"ProcessId":10,"Name":"codex.exe","ExecutablePath":"C:\\a\\codex.exe"},
                      {"ProcessId":11,"Name":"ChatGPT.exe","ExecutablePath":"C:\\b\\ChatGPT.exe"}]"#;
        let parsed = parse_process_json(arr);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].pid, 10);

        let single = r#"{"ProcessId":10,"Name":"codex.exe","ExecutablePath":null}"#;
        let parsed = parse_process_json(single);
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].path.is_none());

        assert!(parse_process_json("").is_empty());
        assert!(parse_process_json("not json").is_empty());
    }
}
