//! Perfis de conta: salvar, listar, trocar e remover, com backup e verificação.

use anyhow::{bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::auth::{self, Identity};
use super::paths::{is_safe_slug, slugify, CodexPaths};

/// Quantos backups manter na pasta `_backups`.
pub const BACKUP_LIMIT: usize = 30;

/// Registro de metadados dos perfis (fonte para exibição).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Registry {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub profiles: Vec<ProfileMeta>,
}

/// Metadados de um perfil salvo.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMeta {
    pub slug: String,
    pub display_name: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub plan: Option<String>,
    pub added_at: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
}

/// Entrada de listagem (metadados + estado no disco).
#[derive(Debug, Clone, Serialize)]
pub struct ProfileEntry {
    pub slug: String,
    pub display_name: String,
    pub email: Option<String>,
    pub account_id: Option<String>,
    pub plan: Option<String>,
    pub added_at: String,
    pub last_used_at: Option<String>,
    pub has_file: bool,
    pub is_active: bool,
}

/// Resultado de uma troca de conta.
#[derive(Debug, Clone, Serialize)]
pub struct SwitchOutcome {
    pub target_slug: String,
    /// `false` quando a conta alvo já estava ativa (apenas sincroniza tokens).
    pub switched: bool,
    /// Caminho do backup do auth.json anterior, quando houve troca.
    pub backup: Option<String>,
    /// Perfil cujos tokens foram re-sincronizados a partir do auth.json vivo.
    pub synced_slug: Option<String>,
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Carrega o registro de perfis (vazio quando ainda não existe).
pub fn load_registry(paths: &CodexPaths) -> Result<Registry> {
    let file = paths.registry_file();
    if !file.exists() {
        return Ok(Registry {
            version: 1,
            profiles: Vec::new(),
        });
    }
    let content =
        fs::read_to_string(&file).with_context(|| format!("falha lendo {}", file.display()))?;
    let mut reg: Registry = serde_json::from_str(&content)
        .with_context(|| format!("registry.json ilegível em {}", file.display()))?;
    if reg.version == 0 {
        reg.version = 1;
    }
    Ok(reg)
}

/// Grava o registro de perfis.
pub fn save_registry(paths: &CodexPaths, reg: &Registry) -> Result<()> {
    let content = serde_json::to_string_pretty(reg)?;
    auth::write_bytes_atomic(&paths.registry_file(), content.as_bytes())?;
    Ok(())
}

/// Lê a identidade de um arquivo de perfil (auth.json copiado).
pub fn read_profile_identity(path: &Path) -> Option<Identity> {
    let content = fs::read_to_string(path).ok()?;
    let auth: auth::AuthDotJson = serde_json::from_str(&content).ok()?;
    Some(auth.identity())
}

/// Descobre qual perfil corresponde ao login ativo (por account_id, depois email).
pub fn active_slug(paths: &CodexPaths, reg: &Registry) -> Result<Option<String>> {
    let auth_file = paths.auth_file();
    if !auth_file.exists() {
        return Ok(None);
    }
    let Some(live) = auth::read_auth_file(&auth_file)? else {
        return Ok(None);
    };
    let live_id = live.identity();

    if let Some(account_id) = live_id.account_id.as_deref().filter(|s| !s.is_empty()) {
        for meta in &reg.profiles {
            if !is_safe_slug(&meta.slug) {
                continue;
            }
            if meta.account_id.as_deref() == Some(account_id) {
                return Ok(Some(meta.slug.clone()));
            }
            if meta.account_id.is_none() {
                if let Some(id) = read_profile_identity(&paths.profile_file(&meta.slug)) {
                    if id.account_id.as_deref() == Some(account_id) {
                        return Ok(Some(meta.slug.clone()));
                    }
                }
            }
        }
    }

    if let Some(email) = live_id.email.as_deref().filter(|s| !s.is_empty()) {
        for meta in &reg.profiles {
            if !is_safe_slug(&meta.slug) {
                continue;
            }
            match meta.email.as_deref() {
                Some(meta_email) if meta_email.eq_ignore_ascii_case(email) => {
                    return Ok(Some(meta.slug.clone()));
                }
                None => {
                    if let Some(id) = read_profile_identity(&paths.profile_file(&meta.slug)) {
                        if id
                            .email
                            .as_deref()
                            .map(|e| e.eq_ignore_ascii_case(email))
                            .unwrap_or(false)
                        {
                            return Ok(Some(meta.slug.clone()));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    Ok(None)
}

/// Lista os perfis: registro + arquivos órfãos encontrados no disco.
pub fn list_profiles(paths: &CodexPaths) -> Result<Vec<ProfileEntry>> {
    let reg = load_registry(paths)?;
    let active = active_slug(paths, &reg)?;
    let mut entries: Vec<ProfileEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for meta in &reg.profiles {
        if !is_safe_slug(&meta.slug) {
            continue;
        }
        let file = paths.profile_file(&meta.slug);
        let has_file = file.exists();
        let mut email = meta.email.clone();
        let mut account_id = meta.account_id.clone();
        let mut plan = meta.plan.clone();
        if has_file {
            if let Some(id) = read_profile_identity(&file) {
                if email.is_none() {
                    email = id.email;
                }
                if account_id.is_none() {
                    account_id = id.account_id;
                }
                if plan.is_none() {
                    plan = id.plan;
                }
            }
        }
        seen.insert(meta.slug.clone());
        entries.push(ProfileEntry {
            slug: meta.slug.clone(),
            display_name: meta.display_name.clone(),
            email,
            account_id,
            plan,
            added_at: meta.added_at.clone(),
            last_used_at: meta.last_used_at.clone(),
            has_file,
            is_active: active.as_deref() == Some(meta.slug.as_str()),
        });
    }

    if let Ok(read_dir) = fs::read_dir(paths.profiles_dir()) {
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let Some(slug) = name.strip_suffix(".auth.json") else {
                continue;
            };
            let slug = slug.to_string();
            if !is_safe_slug(&slug) {
                continue;
            }
            if seen.contains(&slug) {
                continue;
            }
            let id = read_profile_identity(&entry.path());
            entries.push(ProfileEntry {
                display_name: slug.clone(),
                email: id.as_ref().and_then(|i| i.email.clone()),
                account_id: id.as_ref().and_then(|i| i.account_id.clone()),
                plan: id.as_ref().and_then(|i| i.plan.clone()),
                slug: slug.clone(),
                added_at: String::new(),
                last_used_at: None,
                has_file: true,
                is_active: active.as_deref() == Some(slug.as_str()),
            });
        }
    }

    entries.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
    });
    Ok(entries)
}

fn upsert_meta(reg: &mut Registry, mut meta: ProfileMeta) {
    if let Some(existing) = reg.profiles.iter_mut().find(|p| p.slug == meta.slug) {
        meta.added_at = existing.added_at.clone();
        meta.last_used_at = existing.last_used_at.clone();
        *existing = meta;
    } else {
        reg.profiles.push(meta);
    }
}

fn store_profile_bytes(
    paths: &CodexPaths,
    slug: &str,
    bytes: &[u8],
    display_name: &str,
    overwrite: bool,
) -> Result<ProfileMeta> {
    let name = display_name.trim();
    if name.is_empty() {
        bail!("dê um nome para a conta");
    }
    let auth_parsed: auth::AuthDotJson =
        serde_json::from_slice(bytes).context("conteúdo não é um auth.json válido do Codex")?;
    if !auth_parsed.has_chatgpt_tokens() && auth_parsed.api_key().is_none() {
        bail!("auth.json sem tokens nem chave de API");
    }

    let mut reg = load_registry(paths)?;
    let slug = slug.to_string();
    if reg.profiles.iter().any(|p| p.slug == slug) && !overwrite {
        let existing = reg
            .profiles
            .iter()
            .find(|p| p.slug == slug)
            .map(|p| p.display_name.clone())
            .unwrap_or_else(|| slug.clone());
        bail!("já existe um perfil chamado \"{existing}\"");
    }

    auth::write_bytes_atomic(&paths.profile_file(&slug), bytes)?;
    let id = auth_parsed.identity();
    let meta = ProfileMeta {
        slug: slug.clone(),
        display_name: name.to_string(),
        email: id.email,
        account_id: id.account_id,
        plan: id.plan,
        added_at: now_rfc3339(),
        last_used_at: None,
    };
    upsert_meta(&mut reg, meta.clone());
    save_registry(paths, &reg)?;
    Ok(meta)
}

/// Salva o login atual (auth.json vivo) como um perfil novo.
pub fn save_current(
    paths: &CodexPaths,
    display_name: &str,
    overwrite: bool,
) -> Result<ProfileMeta> {
    let auth_path = paths.auth_file();
    if !auth_path.exists() {
        bail!("nenhum login ativo encontrado (auth.json ausente)");
    }
    let bytes =
        fs::read(&auth_path).with_context(|| format!("falha lendo {}", auth_path.display()))?;
    let slug = slugify(display_name);
    store_profile_bytes(paths, &slug, &bytes, display_name, overwrite)
}

/// Importa um auth.json externo (ex.: login isolado em CODEX_HOME temporário).
pub fn import_file(
    paths: &CodexPaths,
    source: &Path,
    display_name: &str,
    overwrite: bool,
) -> Result<ProfileMeta> {
    if !source.exists() {
        bail!("arquivo de origem não encontrado: {}", source.display());
    }
    let bytes = fs::read(source).with_context(|| format!("falha lendo {}", source.display()))?;
    let slug = slugify(display_name);
    store_profile_bytes(paths, &slug, &bytes, display_name, overwrite)
}

/// Copia o auth.json vivo para o arquivo do perfil (captura tokens rotacionados).
///
/// Falha segura: se o auth.json vivo não parseia (gravação em andamento,
/// truncado), o perfil fica como está — o último estado bom nunca é
/// sobrescrito por lixo.
fn sync_profile_from_live(paths: &CodexPaths, slug: &str) -> Result<bool> {
    let live_path = paths.auth_file();
    let profile_path = paths.profile_file(slug);
    if !live_path.exists() || !profile_path.exists() {
        return Ok(false);
    }
    let bytes = fs::read(&live_path)?;
    if serde_json::from_slice::<auth::AuthDotJson>(&bytes).is_err() {
        return Ok(false);
    }
    let current = fs::read(&profile_path)?;
    if bytes == current {
        return Ok(false);
    }
    auth::write_bytes_atomic(&profile_path, &bytes)?;
    Ok(true)
}

/// Faz backup do auth.json vivo na pasta `_backups` e poda os antigos.
pub fn backup_current(paths: &CodexPaths, label: Option<&str>) -> Result<Option<PathBuf>> {
    let auth_path = paths.auth_file();
    if !auth_path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&auth_path)?;
    let hash = auth::sha256_hex(&bytes);
    let ts = Utc::now().format("%Y%m%d-%H%M%S");
    let label_part = label.map(|l| format!("-{l}")).unwrap_or_default();
    let name = format!(
        "auth-backup-{ts}{label_part}-{}.json",
        &hash[..8.min(hash.len())]
    );
    let dir = paths.backups_dir();
    fs::create_dir_all(&dir).with_context(|| format!("falha criando {}", dir.display()))?;
    let target = dir.join(name);
    auth::write_bytes_atomic(&target, &bytes)?;
    prune_backups(&dir, BACKUP_LIMIT);
    Ok(Some(target))
}

fn prune_backups(dir: &Path, keep: usize) {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = read_dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            (name.starts_with("auth-backup-") && name.ends_with(".json")).then_some(name)
        })
        .collect();
    if names.len() <= keep {
        return;
    }
    names.sort();
    let remove_count = names.len() - keep;
    for name in names.into_iter().take(remove_count) {
        let _ = fs::remove_file(dir.join(name));
    }
}

/// Troca a conta ativa para o perfil indicado.
///
/// Passos, nesta ordem:
/// 1. valida o perfil alvo;
/// 2. se a conta alvo já está ativa, apenas re-sincroniza os tokens salvos;
/// 3. faz backup do auth.json atual;
/// 4. re-salva o perfil da conta que está saindo com os tokens vivos (rotação);
/// 5. grava o perfil alvo no auth.json com verificação de hash;
/// 6. atualiza o registro.
pub fn switch_to(paths: &CodexPaths, slug: &str) -> Result<SwitchOutcome> {
    if !is_safe_slug(slug) {
        bail!("nome interno de perfil inválido");
    }
    let mut reg = load_registry(paths)?;
    let Some(_meta) = reg.profiles.iter().find(|p| p.slug == slug).cloned() else {
        bail!("perfil não encontrado: {slug}");
    };
    let target_path = paths.profile_file(slug);
    if !target_path.exists() {
        bail!(
            "arquivo do perfil não encontrado: {}",
            target_path.display()
        );
    }
    let target_bytes = fs::read(&target_path)?;
    let _check: auth::AuthDotJson =
        serde_json::from_slice(&target_bytes).context("arquivo do perfil está corrompido")?;

    let active = active_slug(paths, &reg)?;

    if active.as_deref() == Some(slug) {
        // Já é a conta ativa: só sincroniza tokens rotacionados de volta ao perfil.
        let synced = sync_profile_from_live(paths, slug)?;
        if synced {
            if let Some(meta) = reg.profiles.iter_mut().find(|p| p.slug == slug) {
                meta.last_used_at = Some(now_rfc3339());
            }
            save_registry(paths, &reg)?;
        }
        return Ok(SwitchOutcome {
            target_slug: slug.to_string(),
            switched: false,
            backup: None,
            synced_slug: synced.then(|| slug.to_string()),
        });
    }

    let backup = backup_current(paths, active.as_deref())?;

    let synced_slug = if let Some(prev) = active.as_deref() {
        sync_profile_from_live(paths, prev)?;
        Some(prev.to_string())
    } else {
        None
    };

    auth::write_bytes_atomic(&paths.auth_file(), &target_bytes)?;

    if let Some(meta) = reg.profiles.iter_mut().find(|p| p.slug == slug) {
        meta.last_used_at = Some(now_rfc3339());
    }
    save_registry(paths, &reg)?;

    Ok(SwitchOutcome {
        target_slug: slug.to_string(),
        switched: true,
        backup: backup.map(|p| p.to_string_lossy().to_string()),
        synced_slug,
    })
}

/// Remove um perfil (com cópia de segurança do arquivo antes de apagar).
pub fn remove_profile(paths: &CodexPaths, slug: &str) -> Result<()> {
    if !is_safe_slug(slug) {
        bail!("nome interno de perfil inválido");
    }
    let mut reg = load_registry(paths)?;
    let exists = reg.profiles.iter().any(|p| p.slug == slug);
    if !exists {
        bail!("perfil não encontrado: {slug}");
    }
    let file = paths.profile_file(slug);
    if file.exists() {
        let bytes = fs::read(&file)?;
        let ts = Utc::now().format("%Y%m%d-%H%M%S");
        let removed = paths
            .backups_dir()
            .join(format!("removed-{slug}-{ts}.json"));
        if let Err(err) = auth::write_bytes_atomic(&removed, &bytes) {
            eprintln!("aviso: não consegui salvar cópia do perfil removido: {err:#}");
        }
        fs::remove_file(&file).with_context(|| format!("falha removendo {}", file.display()))?;
    }
    reg.profiles.retain(|p| p.slug != slug);
    save_registry(paths, &reg)?;
    Ok(())
}

/// Renomeia o nome de exibição de um perfil (o slug/arquivo permanece).
pub fn rename_profile(
    paths: &CodexPaths,
    slug: &str,
    new_display_name: &str,
) -> Result<ProfileMeta> {
    if !is_safe_slug(slug) {
        bail!("nome interno de perfil inválido");
    }
    let name = new_display_name.trim();
    if name.is_empty() {
        bail!("o nome não pode ficar vazio");
    }
    let mut reg = load_registry(paths)?;
    let Some(meta) = reg.profiles.iter_mut().find(|p| p.slug == slug) else {
        bail!("perfil não encontrado: {slug}");
    };
    meta.display_name = name.to_string();
    let updated = meta.clone();
    save_registry(paths, &reg)?;
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::auth::test_util::{auth_json, claims};
    use tempfile::TempDir;

    fn setup() -> (TempDir, CodexPaths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = CodexPaths::at(tmp.path().to_path_buf());
        (tmp, paths)
    }

    fn write_live(paths: &CodexPaths, text: &str) {
        std::fs::write(paths.auth_file(), text).unwrap();
    }

    #[test]
    fn list_profiles_ignores_unsafe_registry_slugs() {
        let (_tmp, paths) = setup();
        std::fs::create_dir_all(paths.profiles_dir()).unwrap();
        std::fs::write(
            paths.registry_file(),
            r#"{
                "version": 1,
                "profiles": [{
                    "slug": "../escape",
                    "display_name": "Perfil inválido",
                    "added_at": "2026-09-24T00:00:00Z"
                }]
            }"#,
        )
        .unwrap();

        assert!(list_profiles(&paths).unwrap().is_empty());
    }

    fn account_a(refresh: &str) -> String {
        auth_json(
            claims("a@x.com", "acc-a", "plus"),
            "access-a",
            refresh,
            "acc-a",
        )
    }

    fn account_b(refresh: &str) -> String {
        auth_json(
            claims("b@x.com", "acc-b", "pro"),
            "access-b",
            refresh,
            "acc-b",
        )
    }

    #[test]
    fn save_current_registers_profile_with_identity() {
        let (_t, paths) = setup();
        write_live(&paths, &account_a("refresh-a"));

        let meta = save_current(&paths, "Conta A", false).unwrap();
        assert_eq!(meta.slug, "conta-a");
        assert_eq!(meta.email.as_deref(), Some("a@x.com"));
        assert_eq!(meta.account_id.as_deref(), Some("acc-a"));
        assert_eq!(meta.plan.as_deref(), Some("plus"));

        let live = std::fs::read(paths.auth_file()).unwrap();
        let stored = std::fs::read(paths.profile_file("conta-a")).unwrap();
        assert_eq!(live, stored);

        let list = list_profiles(&paths).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].has_file);
        assert!(list[0].is_active);
    }

    #[test]
    fn save_current_refuses_duplicate_without_overwrite() {
        let (_t, paths) = setup();
        write_live(&paths, &account_a("refresh-a"));
        save_current(&paths, "Conta A", false).unwrap();

        let err = save_current(&paths, "conta a", false).unwrap_err();
        assert!(err.to_string().contains("já existe"));

        save_current(&paths, "Conta A", true).unwrap();
        assert_eq!(list_profiles(&paths).unwrap().len(), 1);
    }

    #[test]
    fn switch_to_writes_target_backs_up_previous_and_syncs_rotated_tokens() {
        let (_t, paths) = setup();
        write_live(&paths, &account_a("refresh-a"));
        save_current(&paths, "Conta A", false).unwrap();

        let b_src = paths.codex_home.join("b-source.json");
        std::fs::write(&b_src, account_b("refresh-b")).unwrap();
        import_file(&paths, &b_src, "Conta B", false).unwrap();
        std::fs::remove_file(&b_src).unwrap();

        // Codex usou a conta A e rotacionou o refresh_token no auth.json vivo.
        write_live(&paths, &account_a("refresh-a-rotated"));

        let out = switch_to(&paths, "conta-b").unwrap();
        assert!(out.switched);
        assert!(out.backup.is_some());
        assert_eq!(out.synced_slug.as_deref(), Some("conta-a"));

        // auth.json agora é a conta B, byte a byte.
        let live = std::fs::read(paths.auth_file()).unwrap();
        let b_stored = std::fs::read(paths.profile_file("conta-b")).unwrap();
        assert_eq!(live, b_stored);

        // O perfil A guarda os tokens rotacionados.
        let a_stored = std::fs::read_to_string(paths.profile_file("conta-a")).unwrap();
        assert!(a_stored.contains("refresh-a-rotated"));

        // O backup guarda o estado vivo anterior à troca.
        let backup = std::fs::read_to_string(out.backup.unwrap()).unwrap();
        assert!(backup.contains("refresh-a-rotated"));
        assert!(backup.contains("acc-a"));

        // A conta ativa agora é a B.
        let reg = load_registry(&paths).unwrap();
        assert_eq!(
            active_slug(&paths, &reg).unwrap().as_deref(),
            Some("conta-b")
        );
        let entry_b = list_profiles(&paths)
            .unwrap()
            .into_iter()
            .find(|p| p.slug == "conta-b")
            .unwrap();
        assert!(entry_b.is_active);
        assert!(entry_b.last_used_at.is_some());
    }

    #[test]
    fn switch_to_active_account_syncs_tokens_without_backup() {
        let (_t, paths) = setup();
        write_live(&paths, &account_a("refresh-a"));
        save_current(&paths, "Conta A", false).unwrap();

        write_live(&paths, &account_a("refresh-a-rotated"));

        let out = switch_to(&paths, "conta-a").unwrap();
        assert!(!out.switched);
        assert!(out.backup.is_none());
        assert_eq!(out.synced_slug.as_deref(), Some("conta-a"));

        let stored = std::fs::read_to_string(paths.profile_file("conta-a")).unwrap();
        assert!(stored.contains("refresh-a-rotated"));
    }

    #[test]
    fn switch_to_unknown_slug_errors() {
        let (_t, paths) = setup();
        write_live(&paths, &account_a("refresh-a"));
        let err = switch_to(&paths, "nao-existe").unwrap_err();
        assert!(err.to_string().contains("não encontrado"));
    }

    #[test]
    fn backups_are_pruned_to_the_limit() {
        let (_t, paths) = setup();
        write_live(&paths, &account_a("refresh-a"));
        for i in 0..(BACKUP_LIMIT + 5) {
            backup_current(&paths, Some(&format!("l{i}"))).unwrap();
        }
        let count = std::fs::read_dir(paths.backups_dir())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("auth-backup-"))
            .count();
        assert_eq!(count, BACKUP_LIMIT);
    }

    #[test]
    fn remove_profile_keeps_a_safety_copy() {
        let (_t, paths) = setup();
        write_live(&paths, &account_a("refresh-a"));
        save_current(&paths, "Conta A", false).unwrap();

        remove_profile(&paths, "conta-a").unwrap();
        assert!(!paths.profile_file("conta-a").exists());
        assert!(list_profiles(&paths).unwrap().is_empty());

        let kept = std::fs::read_dir(paths.backups_dir())
            .unwrap()
            .flatten()
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("removed-conta-a-")
            });
        assert!(
            kept,
            "cópia de segurança do perfil removido deveria existir"
        );
    }

    #[test]
    fn rename_profile_updates_display_name_only() {
        let (_t, paths) = setup();
        write_live(&paths, &account_a("refresh-a"));
        save_current(&paths, "Conta A", false).unwrap();

        let updated = rename_profile(&paths, "conta-a", "Pessoal").unwrap();
        assert_eq!(updated.display_name, "Pessoal");
        assert_eq!(updated.slug, "conta-a");
        assert!(paths.profile_file("conta-a").exists());
    }

    #[test]
    fn sync_keeps_profile_intact_when_live_auth_is_corrupt() {
        let (_t, paths) = setup();
        write_live(&paths, &account_a("refresh-a"));
        save_current(&paths, "Conta A", false).unwrap();
        let before = std::fs::read(paths.profile_file("conta-a")).unwrap();

        // auth.json vivo truncado (ex.: gravação em andamento): não pode
        // sobrescrever o último estado bom guardado no perfil.
        write_live(&paths, "{ \"tokens\": { \"access");

        let synced = sync_profile_from_live(&paths, "conta-a").unwrap();
        assert!(
            !synced,
            "sync não deve acontecer com auth.json vivo ilegível"
        );

        let after = std::fs::read(paths.profile_file("conta-a")).unwrap();
        assert_eq!(
            before, after,
            "perfil bom não pode ser sobrescrito por lixo"
        );
    }
}
