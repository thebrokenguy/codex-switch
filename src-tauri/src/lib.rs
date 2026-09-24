// codex-switch: alternador de contas do Codex (app + CLI) com cotas na bandeja.

mod app;
mod core;

use std::sync::Mutex;
use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, WindowEvent};

use crate::app::{cleanup_login_on_exit, AppState};
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

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open_item = MenuItem::with_id(app, "open", "Abrir painel", true, None::<&str>)?;
    let refresh_item =
        MenuItem::with_id(app, "refresh", "Atualizar cotas agora", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Sair", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open_item,
            &refresh_item,
            &PredefinedMenuItem::separator(app)?,
            &quit_item,
        ],
    )?;

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
