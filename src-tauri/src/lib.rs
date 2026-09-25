// codex-switch: alternador de contas do Codex (app + CLI) com cotas na bandeja.

mod app;
mod core;

use std::sync::Mutex;
use std::time::Duration;

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};

use crate::app::{cleanup_login_on_exit, AccountRow, AppState};
use crate::core::paths::CodexPaths;
use crate::core::settings;
use crate::core::usage;

fn show_main(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn spawn_poller(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Primeira leitura logo após abrir; depois no intervalo configurado.
        tokio::time::sleep(Duration::from_secs(3)).await;
        loop {
            if let Err(e) = app::refresh_and_emit(app.clone()).await {
                eprintln!("aviso: falha atualizando cotas: {e:#}");
            }
            // Dorme em passos curtos e reavalia: assim mudanças de intervalo
            // feitas na UI valem sem reiniciar o app.
            let mut waited = Duration::ZERO;
            loop {
                let step = Duration::from_secs(15);
                tokio::time::sleep(step).await;
                waited += step;
                let target_minutes = {
                    let state = app.state::<AppState>();
                    let guard = state.settings.lock().unwrap();
                    guard.poll_interval_minutes.max(1) as u64
                };
                if waited >= Duration::from_secs(target_minutes * 60) {
                    break;
                }
            }
        }
    });
}

/// Menor percentual restante entre as janelas da conta, com o nome da janela.
fn quota_min(row: &AccountRow) -> Option<(i64, &'static str)> {
    match (row.usage.session_remaining, row.usage.weekly_remaining) {
        (Some(session), Some(weekly)) => {
            if session <= weekly {
                Some((session, "5h"))
            } else {
                Some((weekly, "semana"))
            }
        }
        (Some(session), None) => Some((session, "5h")),
        (None, Some(weekly)) => Some((weekly, "semana")),
        (None, None) => None,
    }
}

fn percent_text(value: i64) -> String {
    format!("{}%", value.clamp(0, 100))
}

/// Texto do tooltip da bandeja: quanto falta para acabar em cada conta.
fn tray_tooltip(rows: &[AccountRow]) -> String {
    if rows.is_empty() {
        return "codex-switch".to_string();
    }
    let mut parts: Vec<String> = rows
        .iter()
        .take(3)
        .map(|row| match quota_min(row) {
            Some((value, window)) => {
                format!("{} {} ({window})", row.display_name, percent_text(value))
            }
            None => format!("{} –", row.display_name),
        })
        .collect();
    if rows.len() > 3 {
        parts.push("…".to_string());
    }
    let mut tip = format!("codex-switch — restante: {}", parts.join(" · "));
    if tip.chars().count() > 120 {
        tip = tip.chars().take(119).collect::<String>() + "…";
    }
    tip
}

/// Linha informativa de cota para o menu da bandeja.
fn quota_menu_label(row: &AccountRow) -> String {
    if row.usage.error.is_some() {
        return format!("{} — cotas indisponíveis", row.display_name);
    }
    let fmt = |value: Option<i64>| match value {
        Some(value) => percent_text(value),
        None => "–".to_string(),
    };
    format!(
        "{} — 5h {} · semana {}",
        row.display_name,
        fmt(row.usage.session_remaining),
        fmt(row.usage.weekly_remaining)
    )
}

/// Monta o menu da bandeja: ações, cotas por conta e sair.
fn tray_menu(app: &AppHandle, rows: &[AccountRow]) -> tauri::Result<Menu<tauri::Wry>> {
    let open_item = MenuItem::with_id(app, "open", "Abrir painel", true, None::<&str>)?;
    let refresh_item =
        MenuItem::with_id(app, "refresh", "Atualizar cotas agora", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
    let mut quota_items = Vec::new();
    for row in rows {
        quota_items.push(MenuItem::with_id(
            app,
            format!("quota-{}", row.slug),
            quota_menu_label(row),
            false,
            None::<&str>,
        )?);
    }
    let quota_sep = PredefinedMenuItem::separator(app)?;
    let end_sep = PredefinedMenuItem::separator(app)?;
    let mut items: Vec<&dyn IsMenuItem<tauri::Wry>> = vec![&open_item, &refresh_item];
    if !quota_items.is_empty() {
        items.push(&quota_sep);
        items.extend(
            quota_items
                .iter()
                .map(|item| item as &dyn IsMenuItem<tauri::Wry>),
        );
    }
    items.push(&end_sep);
    items.push(&quit_item);
    Menu::with_items(app, &items)
}

/// Atualiza tooltip e menu da bandeja com o estado atual das cotas.
pub(crate) fn update_tray_status(app: &AppHandle, rows: &[AccountRow]) {
    let Some(tray) = app.tray_by_id("main-tray") else {
        return;
    };
    let _ = tray.set_tooltip(Some(tray_tooltip(rows)));
    match tray_menu(app, rows) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => eprintln!("aviso: falha atualizando menu da bandeja: {e}"),
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = tray_menu(app, &[])?;
    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("codex-switch")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main(app),
            "refresh" => {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = app::refresh_and_emit(handle).await;
                });
            }
            "quit" => {
                cleanup_login_on_exit(app);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();

            let paths = CodexPaths::discover();
            crate::core::login::cleanup_stale_isolated_homes();
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf()));
            let settings_path = settings::resolve_settings_path(exe_dir, paths.profiles_dir());
            let loaded_settings = settings::load(&settings_path);
            let http = usage::build_client().expect("cliente HTTP");

            app.manage(AppState {
                paths,
                settings_path,
                settings: Mutex::new(loaded_settings),
                http,
                usage: Mutex::new(Default::default()),
                login: Mutex::new(None),
            });

            build_tray(&handle)?;
            spawn_poller(handle);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Fechar a janela esconde para a bandeja; o app segue rodando.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            app::list_accounts,
            app::refresh_usage,
            app::save_current,
            app::switch_account,
            app::codex_processes,
            app::live_identity,
            app::add_account_start,
            app::add_account_status,
            app::add_account_cancel,
            app::remove_account,
            app::rename_account,
            app::get_settings,
            app::set_settings,
            app::get_codex_path,
            app::set_codex_bin_path,
            app::open_profiles_folder,
            app::quit_app,
            app::get_start_with_windows,
            app::set_start_with_windows,
        ])
        .run(tauri::generate_context!())
        .expect("error while running codex-switch");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::UsageView;

    fn row(name: &str, session: Option<i64>, weekly: Option<i64>) -> AccountRow {
        AccountRow {
            slug: "conta".to_string(),
            display_name: name.to_string(),
            email: None,
            plan: None,
            is_active: false,
            has_file: true,
            last_used_at: None,
            usage: UsageView {
                session_remaining: session,
                weekly_remaining: weekly,
                ..Default::default()
            },
        }
    }

    #[test]
    fn tooltip_uses_tightest_window_per_account() {
        let tip = tray_tooltip(&[
            row("pessoal", Some(37), Some(84)),
            row("trabalho", Some(90), Some(12)),
        ]);
        assert_eq!(
            tip,
            "codex-switch — restante: pessoal 37% (5h) · trabalho 12% (semana)"
        );
    }

    #[test]
    fn tooltip_without_usage_reads_dash() {
        let tip = tray_tooltip(&[row("pessoal", None, None)]);
        assert_eq!(tip, "codex-switch — restante: pessoal –");
    }

    #[test]
    fn tooltip_caps_accounts_at_three() {
        let rows = vec![
            row("um", Some(10), None),
            row("dois", Some(20), None),
            row("tres", Some(30), None),
            row("quatro", Some(40), None),
        ];
        let tip = tray_tooltip(&rows);
        assert!(tip.contains("um 10% (5h)"));
        assert!(tip.contains("tres 30% (5h)"));
        assert!(tip.ends_with('…'));
        assert!(!tip.contains("quatro"));
    }

    #[test]
    fn tooltip_is_length_capped() {
        let long_a = "x".repeat(60);
        let long_b = "y".repeat(60);
        let rows = vec![row(&long_a, Some(100), None), row(&long_b, Some(99), None)];
        let tip = tray_tooltip(&rows);
        assert!(tip.chars().count() <= 120);
        assert!(tip.ends_with('…'));
    }

    #[test]
    fn quota_menu_label_lists_both_windows() {
        let label = quota_menu_label(&row("pessoal", Some(37), Some(84)));
        assert_eq!(label, "pessoal — 5h 37% · semana 84%");

        let mut failing = row("pessoal", Some(37), Some(84));
        failing.usage.error = Some("falhou".to_string());
        assert_eq!(quota_menu_label(&failing), "pessoal — cotas indisponíveis");
    }
}
