//! Camada do app Tauri: estado compartilhado, comandos e fluxos (login, troca).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::core::auth::{self, TokenData};
use crate::core::codex_process::{self, ProcInfo};
use crate::core::format;
use crate::core::login;
use crate::core::paths::{slugify, CodexPaths};
use crate::core::profiles::{self, ProfileEntry, ProfileMeta};
use crate::core::settings::{self, Settings};
use crate::core::usage;

/// Estado compartilhado do processo do app.
pub struct AppState {
    pub paths: CodexPaths,
    pub settings_path: PathBuf,
    pub settings: Mutex<Settings>,
    pub http: reqwest::Client,
    pub usage: Mutex<HashMap<String, UsageView>>,
    pub login: Mutex<Option<LoginSession>>,
}

/// Sessão de login isolado em andamento.
pub struct LoginSession {
    home: PathBuf,
    child: Child,
    name: String,
    started: Instant,
}

impl Drop for LoginSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        login::cleanup(&self.home);
    }
}

/// Visão de uso para a UI (textos já formatados).
#[derive(Debug, Clone, Serialize, Default)]
pub struct UsageView {
    pub session_text: String,
    pub weekly_text: String,
    pub session_remaining: Option<i64>,
    pub weekly_remaining: Option<i64>,
    pub error: Option<String>,
    pub fetched_at: Option<i64>,
}

impl UsageView {
    fn err(message: String) -> Self {
        Self {
            session_text: "–".into(),
            weekly_text: "–".into(),
            error: Some(message),
            ..Default::default()
        }
    }

    fn ok(snap: &usage::UsageSnapshot, now: i64) -> Self {
        fn remaining(w: Option<&usage::UsageWindow>, now: i64) -> Option<i64> {
            let w = w?;
            let reset_passed = w
                .resets_at
                .map(|r| now >= format::normalize_reset_epoch(r))
                .unwrap_or(false);
            Some(if reset_passed {
                100
            } else {
                format::remaining_percent(w.used_percent)
            })
        }
        Self {
            session_text: format::format_window_at(snap.session.as_ref(), now),
            weekly_text: format::format_window_at(snap.weekly.as_ref(), now),
            session_remaining: remaining(snap.session.as_ref(), now),
            weekly_remaining: remaining(snap.weekly.as_ref(), now),
            error: None,
            fetched_at: Some(now),
        }
    }
}

/// Uma linha da tabela de contas.
#[derive(Debug, Clone, Serialize)]
pub struct AccountRow {
    pub slug: String,
    pub display_name: String,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub is_active: bool,
    pub has_file: bool,
    pub last_used_at: Option<String>,
    pub usage: UsageView,
}

/// Identidade do login vivo (auth.json atual).
#[derive(Debug, Serialize)]
pub struct IdentityView {
    pub email: Option<String>,
    pub plan: Option<String>,
}

/// Estado da descoberta do executável do Codex para a tela de configurações.
#[derive(Debug, Serialize)]
pub struct CodexPathView {
    pub configured_path: Option<String>,
    pub detected_path: Option<String>,
}

/// Resultado da troca de conta.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SwitchReport {
    /// Há processos abertos e o usuário precisa confirmar o fechamento.
    NeedsConfirmation { processes: Vec<ProcInfo> },
    /// Troca executada.
    Switched {
        switched: bool,
        closed: usize,
        reopened: bool,
        reopen_error: Option<String>,
        backup: Option<String>,
        synced: Option<String>,
    },
}

/// Estado do login isolado (add conta).
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LoginStatus {
    Idle,
    Running,
    Done { profile: ProfileMeta },
    Failed { message: String },
}

fn err_msg<E: std::fmt::Display>(e: E) -> String {
    format!("{e:#}")
}

async fn list_procs() -> Vec<ProcInfo> {
    tauri::async_runtime::spawn_blocking(codex_process::list_codex_processes)
        .await
        .ok()
        .and_then(|r| r.ok())
        .unwrap_or_default()
}

fn build_rows(state: &AppState, entries: &[ProfileEntry]) -> Vec<AccountRow> {
    let cache = state.usage.lock().unwrap();
    entries
        .iter()
        .map(|e| AccountRow {
            slug: e.slug.clone(),
            display_name: e.display_name.clone(),
            email: e.email.clone(),
            plan: e.plan.clone(),
            is_active: e.is_active,
            has_file: e.has_file,
            last_used_at: e.last_used_at.clone(),
            usage: cache.get(&e.slug).cloned().unwrap_or_default(),
        })
        .collect()
}

fn list_accounts_inner(state: &AppState) -> Result<Vec<AccountRow>> {
    let entries = profiles::list_profiles(&state.paths)?;
    Ok(build_rows(state, &entries))
}

/// Renova os tokens de um perfil e grava de volta (rotação preservada).
async fn refresh_and_persist(
    state: &AppState,
    entry: &ProfileEntry,
    tokens: &TokenData,
) -> Result<String> {
    let rt = tokens
        .refresh_token
        .clone()
        .filter(|t| !t.trim().is_empty())
        .context("sem refresh_token")?;
    let refreshed = usage::refresh_tokens(&state.http, &rt).await?;
    let merged = usage::merge_refreshed_tokens(tokens, refreshed);

    let primary = if entry.is_active {
        state.paths.auth_file()
    } else {
        state.paths.profile_file(&entry.slug)
    };
    auth::update_tokens_file(
        &primary,
        &merged.id_token,
        &merged.access_token,
        Some(&merged.refresh_token),
    )
    .with_context(|| format!("falha gravando tokens em {}", primary.display()))?;

    if entry.is_active {
        let prof = state.paths.profile_file(&entry.slug);
        if prof.exists() {
            let _ = auth::update_tokens_file(
                &prof,
                &merged.id_token,
                &merged.access_token,
                Some(&merged.refresh_token),
            );
        }
    }
    Ok(merged.access_token)
}

fn is_unauthorized(e: &anyhow::Error) -> bool {
    e.to_string().contains("(401)")
}

/// Consulta a cota de uma conta (renovando token quando aplicável).
async fn refresh_one(
    state: &AppState,
    entry: &ProfileEntry,
    codex_running: bool,
    now: i64,
) -> UsageView {
    let path = if entry.is_active {
        state.paths.auth_file()
    } else {
        state.paths.profile_file(&entry.slug)
    };
    let auth = match auth::read_auth_file(&path) {
        Ok(Some(a)) => a,
        Ok(None) => return UsageView::err("sem credencial salva".into()),
        Err(e) => return UsageView::err(format!("credencial ilegível: {e:#}")),
    };
    if !auth.has_chatgpt_tokens() {
        return UsageView::err("conta usa chave de API (sem cotas ChatGPT)".into());
    }
    let identity = auth.identity();
    let tokens = auth.tokens.clone().unwrap_or_default();
    let account_id = tokens
        .account_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| identity.account_id.clone());
    let Some(account_id) = account_id else {
        return UsageView::err("conta sem account_id".into());
    };

    let mut access = tokens.access_token.clone();
    let has_rt = tokens
        .refresh_token
        .as_deref()
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false);
    let needs = auth::tokens_need_refresh(&tokens.id_token, &tokens.access_token, now, 120);
    let mut refresh_attempted = false;

    if usage::should_refresh_tokens(entry.is_active, codex_running, needs, has_rt) {
        refresh_attempted = true;
        if let Ok(new_access) = refresh_and_persist(state, entry, &tokens).await {
            access = new_access;
        }
        // falha na renovação não aborta: a consulta abaixo decide (401 etc.)
    }

    let mut result = usage::fetch_usage(&state.http, &access, &account_id).await;
    if let Err(e) = &result {
        if !refresh_attempted
            && is_unauthorized(e)
            && usage::should_refresh_tokens(entry.is_active, codex_running, true, has_rt)
        {
            if let Ok(new_access) = refresh_and_persist(state, entry, &tokens).await {
                result = usage::fetch_usage(&state.http, &new_access, &account_id).await;
            }
        }
    }

    match result {
        Ok(Some(snap)) => UsageView::ok(&snap, now),
        Ok(None) => UsageView::err("sem dados de uso".into()),
        Err(e) => UsageView::err(format!("{e:#}")),
    }
}

/// Reconsulta a cota de todas as contas e atualiza o cache.
pub async fn refresh_all(state: &AppState) -> Result<()> {
    let entries = profiles::list_profiles(&state.paths)?;
    if entries.is_empty() {
        return Ok(());
    }
    let codex_running = !list_procs().await.is_empty();
    let now = chrono::Utc::now().timestamp();
    for entry in &entries {
        let view = refresh_one(state, entry, codex_running, now).await;
        state.usage.lock().unwrap().insert(entry.slug.clone(), view);
    }
    Ok(())
}

/// Reconsulta e emite o evento para a UI (usado pelo loop e pela bandeja).
pub async fn refresh_and_emit(app: AppHandle) -> Result<()> {
    {
        let state = app.state::<AppState>();
        refresh_all(&state).await?;
    }
    let rows = {
        let state = app.state::<AppState>();
        list_accounts_inner(&state)?
    };
    let _ = app.emit("usage-updated", rows);
    Ok(())
}

/// Limpeza do login isolado ao sair (evita processo/pasta órfãos).
pub fn cleanup_login_on_exit(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut guard = state.login.lock().unwrap();
    if let Some(mut s) = guard.take() {
        let _ = s.child.kill();
        let _ = s.child.wait();
        login::cleanup(&s.home);
    }
}

// ---------------------------------------------------------------------------
// Comandos expostos ao frontend
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_accounts(state: State<'_, AppState>) -> Result<Vec<AccountRow>, String> {
    list_accounts_inner(&state).map_err(err_msg)
}

#[tauri::command]
pub async fn refresh_usage(state: State<'_, AppState>) -> Result<Vec<AccountRow>, String> {
    refresh_all(&state).await.map_err(err_msg)?;
    list_accounts_inner(&state).map_err(err_msg)
}

#[tauri::command]
pub fn save_current(
    state: State<'_, AppState>,
    name: String,
    overwrite: bool,
) -> Result<ProfileMeta, String> {
    profiles::save_current(&state.paths, &name, overwrite).map_err(err_msg)
}

#[tauri::command]
pub async fn switch_account(
    state: State<'_, AppState>,
    slug: String,
    close_codex: bool,
) -> Result<SwitchReport, String> {
    let procs = list_procs().await;
    if !procs.is_empty() && !close_codex {
        return Ok(SwitchReport::NeedsConfirmation { processes: procs });
    }

    let mut closed = 0usize;
    let mut desktop_was_running = false;
    if !procs.is_empty() {
        desktop_was_running = procs.iter().any(codex_process::is_desktop_app);
        let (c, _errors) = codex_process::close_codex_processes(&procs);
        closed = c;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let remaining = list_procs().await;
            if remaining.is_empty() {
                break;
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "não consegui fechar todos os processos do Codex ({} restantes)",
                    remaining.len()
                ));
            }
            let _ = codex_process::close_codex_processes(&remaining);
        }
    }

    let outcome = profiles::switch_to(&state.paths, &slug).map_err(err_msg)?;

    let mut reopened = false;
    let mut reopen_error = None;
    let reopen_pref = state.settings.lock().unwrap().reopen_after_switch;
    if desktop_was_running && reopen_pref {
        let configured = state
            .settings
            .lock()
            .unwrap()
            .codex_bin_path
            .clone()
            .map(PathBuf::from);
        let cli = login::find_codex_cli(configured.as_deref());
        match codex_process::reopen_desktop_app(cli.as_deref()) {
            Ok(true) => reopened = true,
            Ok(false) => reopen_error = Some("não encontrei o app do Codex para reabrir".into()),
            Err(e) => reopen_error = Some(format!("falha ao reabrir: {e:#}")),
        }
    }

    Ok(SwitchReport::Switched {
        switched: outcome.switched,
        closed,
        reopened,
        reopen_error,
        backup: outcome.backup,
        synced: outcome.synced_slug,
    })
}

#[tauri::command]
pub async fn codex_processes() -> Result<Vec<ProcInfo>, String> {
    Ok(list_procs().await)
}

#[tauri::command]
pub fn live_identity(state: State<'_, AppState>) -> Result<Option<IdentityView>, String> {
    let auth = auth::read_auth_file(&state.paths.auth_file()).map_err(err_msg)?;
    let Some(auth) = auth else { return Ok(None) };
    let id = auth.identity();
    Ok(Some(IdentityView {
        email: id.email,
        plan: id.plan,
    }))
}

#[tauri::command]
pub fn add_account_start(state: State<'_, AppState>, name: String) -> Result<(), String> {
    let mut guard = state.login.lock().unwrap();
    if guard.is_some() {
        return Err("já existe um login em andamento".into());
    }
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("dê um nome para a conta".into());
    }
    let slug = slugify(trimmed);
    let exists = profiles::list_profiles(&state.paths)
        .map(|l| l.iter().any(|p| p.slug == slug))
        .unwrap_or(false);
    if exists {
        return Err(format!(
            "já existe uma conta chamada \"{trimmed}\" — escolha outro nome (ou remova a antiga antes de refazer o login)"
        ));
    }

    let home = login::prepare_isolated_home().map_err(err_msg)?;
    let configured = state
        .settings
        .lock()
        .unwrap()
        .codex_bin_path
        .clone()
        .map(PathBuf::from);
    let cli = match login::find_codex_cli(configured.as_deref()) {
        Some(c) => c,
        None => {
            login::cleanup(&home);
            return Err("não encontrei o executável do codex (use Config > Selecionar codex.exe ou defina CODEX_SWITCH_CODEX_BIN)".into());
        }
    };
    match login::spawn_login(&cli, &home) {
        Ok(child) => {
            *guard = Some(LoginSession {
                home,
                child,
                name: trimmed.to_string(),
                started: Instant::now(),
            });
            Ok(())
        }
        Err(e) => {
            login::cleanup(&home);
            Err(err_msg(e))
        }
    }
}

#[tauri::command]
pub fn add_account_status(state: State<'_, AppState>) -> Result<LoginStatus, String> {
    let mut guard = state.login.lock().unwrap();
    let Some(session) = guard.as_mut() else {
        return Ok(LoginStatus::Idle);
    };

    let auth_path = login::isolated_auth_path(&session.home);
    if auth_path.exists() {
        // O arquivo pode ainda estar sendo gravado: se não der para ler como
        // auth.json válido, mantém a sessão viva e tenta no próximo poll.
        if !matches!(auth::read_auth_file(&auth_path), Ok(Some(_))) {
            return Ok(LoginStatus::Running);
        }
        let result = profiles::import_file(&state.paths, &auth_path, &session.name, false);
        let _ = session.child.kill();
        let _ = session.child.wait();
        let home = session.home.clone();
        *guard = None;
        login::cleanup(&home);
        return match result {
            Ok(meta) => Ok(LoginStatus::Done { profile: meta }),
            Err(e) => Ok(LoginStatus::Failed {
                message: err_msg(e),
            }),
        };
    }

    if session.started.elapsed() > Duration::from_secs(600) {
        let _ = session.child.kill();
        let _ = session.child.wait();
        let home = session.home.clone();
        *guard = None;
        login::cleanup(&home);
        return Ok(LoginStatus::Failed {
            message: "tempo esgotado — o login não foi concluído".into(),
        });
    }

    match session.child.try_wait() {
        Ok(Some(status)) => {
            let mut detail = String::new();
            if let Some(mut err) = session.child.stderr.take() {
                use std::io::Read;
                let _ = err.read_to_string(&mut detail);
            }
            let home = session.home.clone();
            *guard = None;
            login::cleanup(&home);
            let mut message =
                format!("o processo de login terminou ({status}) sem salvar credencial");
            if !detail.trim().is_empty() {
                message.push_str(&format!(". Detalhe: {}", detail.trim()));
            }
            Ok(LoginStatus::Failed { message })
        }
        Ok(None) => Ok(LoginStatus::Running),
        Err(e) => Ok(LoginStatus::Failed {
            message: format!("erro observando o login: {e}"),
        }),
    }
}

#[tauri::command]
pub fn add_account_cancel(state: State<'_, AppState>) -> Result<(), String> {
    let mut guard = state.login.lock().unwrap();
    if let Some(mut session) = guard.take() {
        let _ = session.child.kill();
        let _ = session.child.wait();
        login::cleanup(&session.home);
    }
    Ok(())
}

#[tauri::command]
pub fn remove_account(state: State<'_, AppState>, slug: String) -> Result<(), String> {
    profiles::remove_profile(&state.paths, &slug).map_err(err_msg)?;
    state.usage.lock().unwrap().remove(&slug);
    Ok(())
}

#[tauri::command]
pub fn rename_account(
    state: State<'_, AppState>,
    slug: String,
    new_name: String,
) -> Result<ProfileMeta, String> {
    profiles::rename_profile(&state.paths, &slug, &new_name).map_err(err_msg)
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_settings(
    state: State<'_, AppState>,
    interval_minutes: u32,
    reopen_after_switch: bool,
) -> Result<Settings, String> {
    let codex_bin_path = state.settings.lock().unwrap().codex_bin_path.clone();
    let updated = Settings {
        poll_interval_minutes: settings::clamp_interval(interval_minutes),
        reopen_after_switch,
        codex_bin_path,
    };
    settings::save(&state.settings_path, &updated).map_err(err_msg)?;
    *state.settings.lock().unwrap() = updated.clone();
    Ok(updated)
}

#[tauri::command]
pub fn get_codex_path(state: State<'_, AppState>) -> CodexPathView {
    let configured_path = state.settings.lock().unwrap().codex_bin_path.clone();
    let configured = configured_path.as_deref().map(Path::new);
    let detected_path = login::find_codex_cli(configured).map(|path| path.display().to_string());
    CodexPathView {
        configured_path,
        detected_path,
    }
}

#[tauri::command]
pub fn set_codex_bin_path(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<Settings, String> {
    let codex_bin_path = match path.filter(|value| !value.trim().is_empty()) {
        Some(raw) => Some(
            login::validate_codex_bin(&raw)
                .map_err(err_msg)?
                .display()
                .to_string(),
        ),
        None => None,
    };
    let mut updated = state.settings.lock().unwrap().clone();
    updated.codex_bin_path = codex_bin_path;
    settings::save(&state.settings_path, &updated).map_err(err_msg)?;
    *state.settings.lock().unwrap() = updated.clone();
    Ok(updated)
}

#[tauri::command]
pub fn open_profiles_folder(state: State<'_, AppState>) -> Result<(), String> {
    let dir = state.paths.profiles_dir();
    std::fs::create_dir_all(&dir).map_err(err_msg)?;
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(&dir)
            .spawn()
            .map_err(err_msg)?;
    }
    Ok(())
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    cleanup_login_on_exit(&app);
    app.exit(0);
}

#[tauri::command]
pub fn get_start_with_windows() -> bool {
    crate::core::startup::is_enabled()
}

#[tauri::command]
pub fn set_start_with_windows(enabled: bool) -> Result<(), String> {
    crate::core::startup::set_enabled(enabled).map_err(err_msg)
}
