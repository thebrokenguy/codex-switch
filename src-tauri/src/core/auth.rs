//! Leitura/escrita do auth.json do Codex e extração de identidade dos tokens.

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// Estrutura do arquivo `auth.json` do Codex (campos relevantes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthDotJson {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
    #[serde(rename = "OPENAI_API_KEY", default)]
    pub openai_api_key: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refresh: Option<String>,
}

/// Tokens OAuth do ChatGPT guardados no auth.json.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenData {
    #[serde(default)]
    pub id_token: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

/// Identidade derivada dos tokens (para exibição e para saber qual conta está ativa).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct Identity {
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub plan: Option<String>,
}

impl AuthDotJson {
    /// Chave de API, quando o arquivo estiver nesse modo.
    pub fn api_key(&self) -> Option<String> {
        self.openai_api_key
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    }

    /// Tem tokens de ChatGPT utilizáveis?
    pub fn has_chatgpt_tokens(&self) -> bool {
        self.tokens
            .as_ref()
            .map(|t| !t.access_token.trim().is_empty() || !t.id_token.trim().is_empty())
            .unwrap_or(false)
    }

    /// Identidade a partir do id_token (email/plano/ids), com fallback para tokens.account_id.
    pub fn identity(&self) -> Identity {
        let mut id = Identity::default();
        if let Some(tokens) = &self.tokens {
            if let Some(payload) = decode_jwt_payload(&tokens.id_token) {
                id = claims_identity(&payload);
            }
            let token_account_id = tokens
                .account_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            if token_account_id.is_some() {
                id.account_id = token_account_id.map(String::from);
            }
        }
        id
    }
}

/// Extrai a identidade relevante de um payload de id_token (namespace de auth do OpenAI).
pub fn claims_identity(payload: &Value) -> Identity {
    let mut id = Identity::default();
    if let Some(email) = payload.get("email").and_then(Value::as_str) {
        id.email = Some(email.to_ascii_lowercase());
    }
    if let Some(ns) = payload.get("https://api.openai.com/auth") {
        id.account_id = ns
            .get("chatgpt_account_id")
            .and_then(Value::as_str)
            .map(String::from);
        id.user_id = ns
            .get("chatgpt_user_id")
            .and_then(Value::as_str)
            .or_else(|| ns.get("user_id").and_then(Value::as_str))
            .map(String::from);
        id.plan = ns
            .get("chatgpt_plan_type")
            .and_then(Value::as_str)
            .map(String::from);
    }
    id
}

/// Decodifica o payload (2ª parte) de um JWT, aceitando base64url com ou sem padding.
pub fn decode_jwt_payload(token: &str) -> Option<Value> {
    let payload_b64 = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()
        .or_else(|| {
            base64::engine::general_purpose::URL_SAFE
                .decode(payload_b64)
                .ok()
        })?;
    serde_json::from_slice(&decoded).ok()
}

/// Claim `exp` de um JWT, quando presente.
pub fn jwt_exp(token: &str) -> Option<i64> {
    decode_jwt_payload(token)?.get("exp")?.as_i64()
}

/// Precisa renovar os tokens? Renova se o id_token expirou/quase (ou é ilegível)
/// ou se o access_token expirou/quase. Access ilegível por si só não força renovação.
pub fn tokens_need_refresh(id_token: &str, access_token: &str, now: i64, skew: i64) -> bool {
    let id_needs = match jwt_exp(id_token) {
        Some(exp) => exp <= now + skew,
        None => true,
    };
    let access_needs = match jwt_exp(access_token) {
        Some(exp) => exp <= now + skew,
        None => false,
    };
    id_needs || access_needs
}

/// SHA-256 em hex minúsculo.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest.as_slice() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Lê um auth.json, se existir.
pub fn read_auth_file(path: &Path) -> Result<Option<AuthDotJson>> {
    if !path.exists() {
        return Ok(None);
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("falha lendo {}", path.display()))?;
    let auth = serde_json::from_str(&content)
        .with_context(|| format!("auth.json ilegível em {}", path.display()))?;
    Ok(Some(auth))
}

/// Escreve bytes de forma atômica (temp + rename) e confere o resultado por SHA-256.
/// Retorna o hash do conteúdo gravado.
pub fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<String> {
    let parent = path.parent().context("caminho sem pasta pai")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("falha criando pasta {}", parent.display()))?;
    let file_name = path
        .file_name()
        .context("caminho sem nome de arquivo")?
        .to_string_lossy()
        .to_string();
    let tmp = parent.join(format!(".{file_name}.tmp{}", std::process::id()));

    let result = (|| -> Result<String> {
        fs::write(&tmp, bytes).with_context(|| format!("falha escrevendo {}", tmp.display()))?;
        let read_back =
            fs::read(&tmp).with_context(|| format!("falha relendo {}", tmp.display()))?;
        let expected = sha256_hex(bytes);
        let actual = sha256_hex(&read_back);
        if expected != actual {
            bail!("verificação por hash falhou: o arquivo gravado não confere");
        }
        fs::rename(&tmp, path).with_context(|| format!("falha movendo para {}", path.display()))?;
        Ok(expected)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
pub(crate) mod test_util {
    use base64::Engine as _;
    use serde_json::Value;

    /// Constrói um JWT sintético (header.payload.sig), sem assinatura real.
    pub fn make_jwt(payload: &Value) -> String {
        let enc = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let header = enc.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let body = enc.encode(serde_json::to_vec(payload).unwrap());
        format!("{header}.{body}.signature")
    }

    /// Claims típicas de um id_token do Codex.
    pub fn claims(email: &str, account_id: &str, plan: &str) -> Value {
        serde_json::json!({
            "email": email,
            "exp": 2_000_000_000i64,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": account_id,
                "chatgpt_plan_type": plan,
                "chatgpt_user_id": format!("user-{account_id}")
            }
        })
    }

    /// Conteúdo completo de um auth.json de teste.
    pub fn auth_json(
        claims_value: Value,
        access_token: &str,
        refresh_token: &str,
        account_id: &str,
    ) -> String {
        serde_json::json!({
            "auth_mode": "chatgpt",
            "OPENAI_API_KEY": Value::Null,
            "tokens": {
                "id_token": make_jwt(&claims_value),
                "access_token": access_token,
                "refresh_token": refresh_token,
                "account_id": account_id
            },
            "last_refresh": "2026-09-18T10:00:00Z"
        })
        .to_string()
    }

    /// JWT apenas com claim `exp`.
    pub fn jwt_with_exp(exp: i64) -> String {
        make_jwt(&serde_json::json!({ "exp": exp }))
    }
}

/// Atualiza os tokens dentro de um auth.json existente, preservando campos
/// desconhecidos, e carimba `last_refresh` com agora.
pub fn update_tokens_file(
    path: &Path,
    id_token: &str,
    access_token: &str,
    refresh_token: Option<&str>,
) -> Result<()> {
    let content =
        fs::read_to_string(path).with_context(|| format!("falha lendo {}", path.display()))?;
    let mut v: Value = serde_json::from_str(&content)
        .with_context(|| format!("auth.json ilegível em {}", path.display()))?;
    let tokens = v
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .context("auth.json sem objeto tokens")?;
    tokens.insert("id_token".into(), Value::String(id_token.to_string()));
    tokens.insert(
        "access_token".into(),
        Value::String(access_token.to_string()),
    );
    if let Some(rt) = refresh_token.filter(|t| !t.trim().is_empty()) {
        tokens.insert("refresh_token".into(), Value::String(rt.to_string()));
    }
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "last_refresh".into(),
            Value::String(Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)),
        );
    }
    let bytes = serde_json::to_vec(&v)?;
    write_bytes_atomic(path, &bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;

    #[test]
    fn update_tokens_file_preserves_unknown_fields_and_stamps_refresh() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auth.json");
        let mut original: Value = serde_json::from_str(&auth_json(
            claims("u@x.com", "acc-1", "plus"),
            "old_access",
            "old_refresh",
            "acc-1",
        ))
        .unwrap();
        original["custom_field"] = serde_json::json!({"keep": true});
        original["tokens"]["extra_token_field"] = serde_json::json!("keepme");
        std::fs::write(&path, serde_json::to_string(&original).unwrap()).unwrap();

        update_tokens_file(&path, "new_id", "new_access", Some("new_refresh")).unwrap();

        let after: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after["tokens"]["id_token"], "new_id");
        assert_eq!(after["tokens"]["access_token"], "new_access");
        assert_eq!(after["tokens"]["refresh_token"], "new_refresh");
        assert_eq!(after["tokens"]["extra_token_field"], "keepme");
        assert_eq!(after["tokens"]["account_id"], "acc-1");
        assert_eq!(after["custom_field"]["keep"], true);
        assert!(after["last_refresh"].as_str().unwrap().len() > 10);
    }

    #[test]
    fn update_tokens_file_errors_without_tokens_object() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("auth.json");
        std::fs::write(&path, r#"{"auth_mode":"chatgpt"}"#).unwrap();
        assert!(update_tokens_file(&path, "a", "b", None).is_err());
    }

    #[test]
    fn identity_reads_email_account_plan_from_id_token() {
        let text = auth_json(claims("User@X.com", "acc-1", "plus"), "acc", "ref", "acc-1");
        let auth: AuthDotJson = serde_json::from_str(&text).unwrap();
        let id = auth.identity();
        assert_eq!(id.email.as_deref(), Some("user@x.com"));
        assert_eq!(id.account_id.as_deref(), Some("acc-1"));
        assert_eq!(id.plan.as_deref(), Some("plus"));
        assert_eq!(id.user_id.as_deref(), Some("user-acc-1"));
        assert!(auth.has_chatgpt_tokens());
        assert!(auth.api_key().is_none());
    }

    #[test]
    fn identity_falls_back_to_token_account_id() {
        let payload = serde_json::json!({
            "email": "b@x.com",
            "https://api.openai.com/auth": {}
        });
        let text = auth_json(payload, "acc", "ref", "acc-2");
        let auth: AuthDotJson = serde_json::from_str(&text).unwrap();
        assert_eq!(auth.identity().account_id.as_deref(), Some("acc-2"));
        assert_eq!(auth.identity().email.as_deref(), Some("b@x.com"));
    }

    #[test]
    fn api_key_file_is_detected() {
        let text = r#"{"auth_mode":"apikey","OPENAI_API_KEY":"  sk-test  ","tokens":null}"#;
        let auth: AuthDotJson = serde_json::from_str(text).unwrap();
        assert_eq!(auth.api_key().as_deref(), Some("sk-test"));
        assert!(!auth.has_chatgpt_tokens());
    }

    #[test]
    fn jwt_exp_reads_exp_claim_and_rejects_garbage() {
        assert_eq!(jwt_exp(&jwt_with_exp(123)), Some(123));
        assert_eq!(jwt_exp("nao-e-um-jwt"), None);
        assert_eq!(jwt_exp("a.!!!invalid-base64!!!.c"), None);
    }

    #[test]
    fn tokens_need_refresh_semantics() {
        let now = 1_000_000;
        let skew = 60;
        let valid = jwt_with_exp(now + 9_999);
        let expired = jwt_with_exp(now - 5);

        assert!(tokens_need_refresh(&expired, &valid, now, skew));
        assert!(!tokens_need_refresh(&valid, &valid, now, skew));
        assert!(tokens_need_refresh(&valid, &expired, now, skew));
        // id_token ilegível força renovação; access_token ilegível não.
        assert!(tokens_need_refresh("nao-e-jwt", &valid, now, skew));
        assert!(!tokens_need_refresh(&valid, "nao-e-jwt", now, skew));
    }

    #[test]
    fn write_bytes_atomic_round_trips_and_replaces() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("sub").join("auth.json");
        let hash1 = write_bytes_atomic(&target, b"one").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"one");
        assert_eq!(hash1.len(), 64);

        let hash2 = write_bytes_atomic(&target, b"two").unwrap();
        assert_ne!(hash1, hash2);
        assert_eq!(std::fs::read(&target).unwrap(), b"two");
        assert_eq!(sha256_hex(b"two"), hash2);
    }

    #[test]
    fn read_auth_file_returns_none_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let none = read_auth_file(&tmp.path().join("nope.json")).unwrap();
        assert!(none.is_none());
    }

    #[test]
    fn decode_jwt_payload_accepts_padded_and_unpadded_base64() {
        let payload = serde_json::json!({"a": 1});
        let jwt = make_jwt(&payload);
        assert_eq!(decode_jwt_payload(&jwt).unwrap()["a"], 1);
    }
}
