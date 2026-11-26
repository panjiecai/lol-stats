mod commands;
mod services;
mod wx_service;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|_| {
            commands::initialize_player_elo_jobs();
            // 微信推送服务跟着应用一起起，退出时由 RunEvent::Exit 回收
            wx_service::start();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_version,
            commands::accept_ready_check,
            commands::cancel_stats_load,
            commands::check_app_update,
            commands::connection_status,
            commands::copy_png_to_clipboard,
            commands::run_external_command,
            commands::dismiss_end_of_game,
            commands::get_chat_status,
            commands::get_game_settings_locked,
            commands::load_champions,
            commands::load_current_ranked_stats,
            commands::load_gameflow_phase,
            commands::load_game_assets,
            commands::load_ranked_stats,
            commands::load_lcu_asset,
            commands::load_lcu_assets,
            commands::load_self_stats,
            commands::load_self_stats_with_progress,
            commands::search_summoner_candidates,
            commands::search_player,
            commands::search_player_with_progress,
            commands::load_match_detail,
            commands::load_player_elo_estimate,
            commands::start_player_elo_estimate_task,
            commands::get_player_elo_estimate_task,
            commands::get_player_elo_queue,
            commands::clear_player_elo_cache,
            commands::debug_external_player_elo_lookup,
            commands::debug_official_api,
            commands::load_live_game,
            commands::load_friends,
            commands::set_chat_availability,
            commands::set_chat_status_message,
            commands::set_game_settings_locked,
            commands::send_alt_left_shortcut,
            commands::wx_service_status,
            commands::restart_wx_service
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_handle, event| {
        // 正常退出时显式回收；即便走到这里之前进程被杀，
        // Job Object 的 KILL_ON_JOB_CLOSE 也会兜住。
        if matches!(event, tauri::RunEvent::Exit) {
            wx_service::stop();
        }
    });
}
