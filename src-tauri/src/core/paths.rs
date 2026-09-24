//! Resolução de caminhos do codex-switch.

use std::path::PathBuf;

/// Raiz de dados do Codex e caminhos derivados que o app usa.
/// `at()` permite injetar uma raiz temporária nos testes.
#[derive(Debug, Clone)]
pub struct CodexPaths {
    pub codex_home: PathBuf,
}

impl CodexPaths {
    /// Descobre a home do Codex: `CODEX_HOME` quando definida, senão `~/.codex`.
    pub fn discover() -> Self {
        let codex_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
            .or_else(|| dirs::home_dir().map(|h| h.join(".codex")))
            .unwrap_or_else(|| PathBuf::from(".codex"));
        Self::at(codex_home)
    }

    /// Constrói os caminhos a partir de uma raiz explícita (testes).
    pub fn at(codex_home: PathBuf) -> Self {
        Self { codex_home }
    }

    /// Arquivo de credencial ativo: `<codex_home>/auth.json`.
    pub fn auth_file(&self) -> PathBuf {
        self.codex_home.join("auth.json")
    }

    /// Pasta dos perfis salvos: `<codex_home>/auth-profiles`.
    pub fn profiles_dir(&self) -> PathBuf {
        self.codex_home.join("auth-profiles")
    }

    /// Pasta de backups: `<codex_home>/auth-profiles/_backups`.
    pub fn backups_dir(&self) -> PathBuf {
        self.profiles_dir().join("_backups")
    }

    /// Registro de metadados dos perfis.
    pub fn registry_file(&self) -> PathBuf {
        self.profiles_dir().join("registry.json")
    }

    /// Arquivo de um perfil específico.
    pub fn profile_file(&self, slug: &str) -> PathBuf {
        self.profiles_dir().join(format!("{slug}.auth.json"))
    }
}

/// Converte um nome de exibição em slug seguro para nome de arquivo.
/// Regras: minúsculas, `[a-z0-9-]`, separadores compactados, sem pontas com `-`.
pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    let mut at_separator = true; // também evita começar com '-'
    for ch in name.chars() {
        match ch {
            'A'..='Z' => {
                out.push(ch.to_ascii_lowercase());
                at_separator = false;
            }
            'a'..='z' | '0'..='9' => {
                out.push(ch);
                at_separator = false;
            }
            _ => {
                if !at_separator && !out.is_empty() {
                    out.push('-');
                    at_separator = true;
                }
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("conta");
    }
    out.truncate(64);
    while out.ends_with('-') {
        out.pop();
    }
    out
}

pub fn is_safe_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 64
        && !slug.starts_with('-')
        && !slug.ends_with('-')
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::slugify;

    #[test]
    fn slugify_normalizes_simple_names() {
        assert_eq!(slugify("Conta A"), "conta-a");
        assert_eq!(slugify("  Pessoal  "), "pessoal");
    }

    #[test]
    fn slugify_replaces_punctuation_with_single_dashes() {
        assert_eq!(slugify("user@example.com"), "user-example-com");
        assert_eq!(slugify("a _ b"), "a-b");
        assert_eq!(slugify("Ação²"), "a-o");
    }

    #[test]
    fn slugify_falls_back_for_empty_names_and_trims_dashes() {
        assert_eq!(slugify("   "), "conta");
        assert_eq!(slugify("---"), "conta");
        assert_eq!(slugify("!@#"), "conta");
    }

    #[test]
    fn safe_slug_rejects_path_and_markup_values() {
        assert!(super::is_safe_slug("conta-a"));
        assert!(!super::is_safe_slug("../escape"));
        assert!(!super::is_safe_slug("Conta A"));
        assert!(!super::is_safe_slug("conta/a"));
        assert!(!super::is_safe_slug(""));
    }
}
