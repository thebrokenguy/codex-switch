//! Cota (rate limits) do ChatGPT: parsing da resposta e cliente HTTP.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::time::Duration;

use super::auth::TokenData;

/// Janela de sessão (5 horas), usada para classificar a janela primária.
pub const SESSION_WINDOW_SECONDS: i64 = 5 * 60 * 60;
/// Janela semanal (7 dias), usada para classificar a janela secundária.
pub const WEEKLY_WINDOW_SECONDS: i64 = 7 * 24 * 60 * 60;

const USAGE_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
const OAUTH_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

/// User-Agent de navegador (mesmo padrão do Codex no Windows) para evitar bloqueio do Cloudflare.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
     AppleWebKit/537.36 (KHTML, like Gecko) \
     Chrome/136.0.0.0 Safari/537.36";

/// Uma janela de rate limit.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct UsageWindow {
    pub used_percent: f64,
    pub window_minutes: Option<i64>,
    pub resets_at: Option<i64>,
}

/// Fotografia do uso de uma conta.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct UsageSnapshot {
    pub plan: Option<String>,
    /// Janela de sessão (5h).
    pub session: Option<UsageWindow>,
    /// Janela semanal (7d).
    pub weekly: Option<UsageWindow>,
    pub credits_balance: Option<String>,
    pub reset_credits_available: Option<i64>,
}

/// Lê a resposta de `/wham/usage`. Retorna `None` quando não há nada útil.
pub fn parse_usage_response(body: &str) -> Result<Option<UsageSnapshot>> {
    let root: Value = serde_json::from_str(body).context("resposta de uso ilegível")?;
    let obj = root.as_object().context("formato inesperado de uso")?;

    let mut snap = UsageSnapshot::default();
    if let Some(plan) = obj.get("plan_type").and_then(Value::as_str) {
        snap.plan = Some(plan.to_string());
    }
    if let Some(rl) = obj.get("rate_limit").and_then(Value::as_object) {
        let primary = rl.get("primary_window").and_then(parse_window);
        let secondary = rl.get("secondary_window").and_then(parse_window);
        let (session, weekly) = normalize_windows(primary, secondary);
        snap.session = session;
        snap.weekly = weekly;
    }
    if let Some(credits) = obj.get("credits").and_then(Value::as_object) {
        if let Some(balance) = credits.get("balance") {
            snap.credits_balance = balance_to_string(balance);
        }
    }
    if let Some(rc) = obj
        .get("rate_limit_reset_credits")
        .and_then(Value::as_object)
    {
        snap.reset_credits_available = rc.get("available_count").and_then(value_as_i64);
    }

    let empty = snap.session.is_none()
        && snap.weekly.is_none()
        && snap.credits_balance.is_none()
        && snap.reset_credits_available.is_none();
    Ok(if empty { None } else { Some(snap) })
}

fn parse_window(v: &Value) -> Option<UsageWindow> {
    let obj = v.as_object()?;
    let used = obj.get("used_percent").and_then(value_as_f64)?;
    let window_minutes = obj
        .get("limit_window_seconds")
        .and_then(value_as_i64)
        .filter(|s| *s > 0)
        .map(|s| (s + 59) / 60);
    let resets_at = obj.get("reset_at").and_then(value_as_i64);
    Some(UsageWindow {
        used_percent: used,
        window_minutes,
        resets_at,
    })
}

fn value_as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn value_as_i64(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn balance_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Normaliza as janelas: sessão (5h) e semanal (7d), mesmo quando o backend
/// promove a semanal a primária ou inverte a ordem.
pub fn normalize_windows(
    primary: Option<UsageWindow>,
    secondary: Option<UsageWindow>,
) -> (Option<UsageWindow>, Option<UsageWindow>) {
    fn is_session(w: &UsageWindow) -> bool {
        w.window_minutes
            .map(|m| m * 60 == SESSION_WINDOW_SECONDS)
            .unwrap_or(false)
    }
    fn is_weekly(w: &UsageWindow) -> bool {
        w.window_minutes
            .map(|m| m * 60 == WEEKLY_WINDOW_SECONDS)
            .unwrap_or(false)
    }

    match (primary, secondary) {
        (Some(p), None) if is_weekly(&p) => (None, Some(p)),
        (None, Some(s)) if is_session(&s) => (Some(s), None),
        (Some(p), Some(s)) if is_weekly(&p) && is_session(&s) => (Some(s), Some(p)),
        (p, s) => (p, s),
    }
}

/// Cliente HTTP com timeout curto (uso recorrente, não pode travar).
pub fn build_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .build()
        .context("falha criando cliente HTTP")
}

/// Busca a cota de uma conta no backend do ChatGPT.
///
/// Erros comuns: `token expirado (401)` e `requisição bloqueada (403)`.
pub async fn fetch_usage(
    client: &reqwest::Client,
    access_token: &str,
    account_id: &str,
) -> Result<Option<UsageSnapshot>> {
    let resp = client
        .get(USAGE_ENDPOINT)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::ACCEPT, "application/json, text/plain, */*")
        .header(
            reqwest::header::AUTHORIZATION,
            format!("Bearer {access_token}"),
        )
        .header("chatgpt-account-id", account_id)
        .header(reqwest::header::ORIGIN, "https://chatgpt.com")
        .header(reqwest::header::REFERER, "https://chatgpt.com/")
        .send()
        .await
        .context("falha na requisição de uso")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::UNAUTHORIZED {
        bail!("token expirado (401)");
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        bail!("requisição bloqueada (403)");
    }
    if !status.is_success() {
        bail!("erro {status} na API de uso");
    }
    parse_usage_response(&body)
}

/// Tokens devolvidos pelo endpoint de renovação do OAuth.
#[derive(Debug, Clone)]
pub struct RefreshedTokens {
    pub id_token: Option<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
}

/// Renova os tokens OAuth a partir do refresh_token.
pub async fn refresh_tokens(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<RefreshedTokens> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", OAUTH_CLIENT_ID),
    ];
    let resp = client
        .post(OAUTH_TOKEN_ENDPOINT)
        .timeout(Duration::from_secs(15))
        .form(&form)
        .send()
        .await
        .context("falha na renovação de token")?;

    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        // Não despeja o corpo cru na interface; extrai só o código de erro, se houver.
        let detail = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v.get("error").and_then(Value::as_str).map(String::from))
            .unwrap_or_default();
        let suffix = if detail.is_empty() {
            String::new()
        } else {
            format!(" ({detail})")
        };
        bail!("renovação falhou: {status}{suffix}");
    }
    let v: Value = serde_json::from_str(&body).context("resposta de renovação ilegível")?;
    let access_token = v
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("resposta de renovação sem access_token")?
        .to_string();

    Ok(RefreshedTokens {
        id_token: v
            .get("id_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from),
        access_token,
        refresh_token: v
            .get("refresh_token")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from),
    })
}

/// Resultado da fusão entre tokens atuais e tokens renovados.
#[derive(Debug, Clone, PartialEq)]
pub struct MergedTokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
}

/// Mescla tokens renovados com os atuais: campos ausentes mantêm o valor antigo
/// (a resposta de renovação pode omitir id_token/refresh_token).
pub fn merge_refreshed_tokens(current: &TokenData, refreshed: RefreshedTokens) -> MergedTokens {
    MergedTokens {
        id_token: refreshed
            .id_token
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| current.id_token.clone()),
        access_token: refreshed.access_token,
        refresh_token: refreshed
            .refresh_token
            .filter(|t| !t.trim().is_empty())
            .or_else(|| current.refresh_token.clone())
            .unwrap_or_default(),
    }
}

/// Decide se vale renovar os tokens antes de consultar o uso.
///
/// Regra: só renova quando precisar e existir refresh_token; a conta ativa com
/// o Codex em execução fica de fora (o próprio Codex cuida da rotação dela).
pub fn should_refresh_tokens(
    is_active: bool,
    codex_running: bool,
    needs_refresh: bool,
    has_refresh_token: bool,
) -> bool {
    needs_refresh && has_refresh_token && !(is_active && codex_running)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_decision_respects_active_account_and_codex_running() {
        assert!(should_refresh_tokens(false, false, true, true));
        assert!(should_refresh_tokens(false, true, true, true));
        assert!(should_refresh_tokens(true, false, true, true));
        assert!(!should_refresh_tokens(true, true, true, true));
        assert!(!should_refresh_tokens(true, true, false, true));
        assert!(!should_refresh_tokens(true, false, false, true));
        assert!(!should_refresh_tokens(false, false, true, false));
        assert!(!should_refresh_tokens(true, false, true, false));
    }

    #[test]
    fn parses_legacy_session_and_weekly_windows() {
        let body = r#"{
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {"used_percent": 27.0, "limit_window_seconds": 18000, "reset_at": 1800000000},
                "secondary_window": {"used_percent": 82.0, "limit_window_seconds": 604800, "reset_at": 1800000000}
            }
        }"#;
        let snap = parse_usage_response(body).unwrap().unwrap();
        assert_eq!(snap.plan.as_deref(), Some("plus"));
        assert_eq!(snap.session.unwrap().used_percent, 27.0);
        assert_eq!(snap.weekly.unwrap().used_percent, 82.0);
    }

    #[test]
    fn promotes_weekly_only_primary_window() {
        let body = r#"{
            "rate_limit": {
                "primary_window": {"used_percent": 35.0, "limit_window_seconds": 604800, "reset_at": 1800000000}
            }
        }"#;
        let snap = parse_usage_response(body).unwrap().unwrap();
        assert!(snap.session.is_none());
        assert_eq!(snap.weekly.unwrap().used_percent, 35.0);
    }

    #[test]
    fn fixes_reversed_window_order() {
        let body = r#"{
            "rate_limit": {
                "primary_window": {"used_percent": 82.0, "limit_window_seconds": 604800, "reset_at": 1},
                "secondary_window": {"used_percent": 27.0, "limit_window_seconds": 18000, "reset_at": 2}
            }
        }"#;
        let snap = parse_usage_response(body).unwrap().unwrap();
        assert_eq!(snap.session.unwrap().used_percent, 27.0);
        assert_eq!(snap.weekly.unwrap().used_percent, 82.0);
    }

    #[test]
    fn keeps_unknown_windows_by_position() {
        let body = r#"{
            "rate_limit": {
                "primary_window": {"used_percent": 11.0, "limit_window_seconds": 3600, "reset_at": 1},
                "secondary_window": {"used_percent": 22.0, "limit_window_seconds": 2592000, "reset_at": 2}
            }
        }"#;
        let snap = parse_usage_response(body).unwrap().unwrap();
        assert_eq!(snap.session.unwrap().used_percent, 11.0);
        assert_eq!(snap.weekly.unwrap().used_percent, 22.0);
    }

    #[test]
    fn accepts_percent_as_string_and_reads_credits() {
        let body = r#"{
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {"used_percent": "55.5", "limit_window_seconds": 18000, "reset_at": "1800000000"}
            },
            "credits": {"has_credits": true, "unlimited": false, "balance": "12.50"},
            "rate_limit_reset_credits": {"available_count": 2}
        }"#;
        let snap = parse_usage_response(body).unwrap().unwrap();
        assert_eq!(snap.session.unwrap().used_percent, 55.5);
        assert_eq!(snap.credits_balance.as_deref(), Some("12.50"));
        assert_eq!(snap.reset_credits_available, Some(2));
    }

    #[test]
    fn empty_payload_returns_none() {
        assert!(parse_usage_response("{}").unwrap().is_none());
        assert!(parse_usage_response(r#"{"rate_limit": null}"#)
            .unwrap()
            .is_none());
    }

    #[test]
    fn merge_keeps_current_fields_when_refresh_omits_them() {
        let current = TokenData {
            id_token: "old-id".into(),
            access_token: "old-access".into(),
            refresh_token: Some("old-refresh".into()),
            account_id: Some("acc".into()),
        };
        let merged = merge_refreshed_tokens(
            &current,
            RefreshedTokens {
                id_token: None,
                access_token: "new-access".into(),
                refresh_token: None,
            },
        );
        assert_eq!(merged.id_token, "old-id");
        assert_eq!(merged.access_token, "new-access");
        assert_eq!(merged.refresh_token, "old-refresh");
    }

    #[test]
    fn merge_uses_rotated_refresh_token_when_returned() {
        let current = TokenData {
            id_token: "old-id".into(),
            access_token: "old-access".into(),
            refresh_token: Some("old-refresh".into()),
            account_id: Some("acc".into()),
        };
        let merged = merge_refreshed_tokens(
            &current,
            RefreshedTokens {
                id_token: Some("new-id".into()),
                access_token: "new-access".into(),
                refresh_token: Some("rotated".into()),
            },
        );
        assert_eq!(merged.id_token, "new-id");
        assert_eq!(merged.refresh_token, "rotated");
    }
}
