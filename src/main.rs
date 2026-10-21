#![windows_subsystem = "windows"]
pub use peripheral_desk::{
    adapters, audio_fx, audio_lighting, backend, configuration, controls, devices, hid, model,
    plugin_manager, profiles, protocol, storage,
};
mod configuration_ui;
mod controls_ui;
mod generic_lighting_ui;
mod lighting_ui;
mod paint;
mod plugin_ui;
mod preset_ui;
mod resident_macro_ui;
mod ui;
mod update_ui;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args
        .get(1)
        .is_some_and(|arg| arg == "--check-g2-components")
    {
        let result = peripheral_desk::audio_component_install::check_installed_bundle();
        println!(
            "{}",
            match result {
                Ok(count) => serde_json::json!({"ok":true,"components":count}),
                Err(error) => serde_json::json!({"ok":false,"error":error}),
            }
        );
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--install-g2-audio") {
        std::process::exit(peripheral_desk::audio_component_install::worker_main());
    }
    if args.get(1).is_some_and(|a| a == "--restart-after") {
        let result = args
            .get(2)
            .and_then(|v| v.parse::<u32>().ok())
            .ok_or_else(|| "缺少重启来源进程".to_string())
            .and_then(peripheral_desk::core_update::wait_for_restart_parent);
        if let Err(error) = result {
            ui::message(&format!("自动重启未完成：{error}"));
            return;
        }
    }
    if args.get(1).is_some_and(|a| a == "--finish-core-update") {
        let result = args
            .get(2)
            .ok_or_else(|| "缺少更新任务".to_string())
            .and_then(|p| peripheral_desk::core_update::finish_update(std::path::Path::new(p)));
        if result.is_err() {
            std::process::exit(1)
        }
        return;
    }
    if args.get(1).is_some_and(|a| a == "--self-check") {
        println!(
            "{}",
            serde_json::json!({"appVersion":env!("CARGO_PKG_VERSION"),"abi":plugin_manager::ABI_VERSION,"updateProtocol":1})
        );
        return;
    }

    if args.get(1).is_some_and(|arg| arg == "--plugin-probe") {
        if let (Some(path), Some(hash)) = (args.get(2), args.get(3)) {
            plugin_manager::probe_worker_main(std::path::Path::new(path), hash);
        } else {
            std::process::exit(2);
        }
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--plugin-worker") {
        if let Some(key) = args.get(2) {
            plugin_manager::worker_main(key);
        } else {
            std::process::exit(2);
        }
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "--audio-fx-worker") {
        audio_fx::worker_main();
        return;
    }
    if args.len() >= 4 && args[1] == "--driver" {
        let result: Result<serde_json::Value, String> = {
            let action = &args[2];
            let value = &args[3];
            match action.as_str() {
                "install" => plugin_manager::install(value)
                    .and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string())),
                "offline" => plugin_manager::install_offline(std::path::Path::new(value))
                    .and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string())),
                "rollback" => plugin_manager::rollback(value)
                    .and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string())),
                "uninstall" => plugin_manager::uninstall(value)
                    .map(|_| serde_json::json!({"removed":value,"configurationRetained":true})),
                "refresh" => plugin_manager::refresh_catalog()
                    .and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string())),
                "catalog" => plugin_manager::catalog()
                    .and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string())),
                "import-catalog" => plugin_manager::import_catalog(std::path::Path::new(value))
                    .and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string())),
                _ => Err("未知驱动操作".into()),
            }
        };
        let failed = result.is_err();
        let reply = match result {
            Ok(v) => serde_json::json!({"ok":true,"value":v}),
            Err(e) => serde_json::json!({"ok":false,"error":e}),
        };
        let bytes = serde_json::to_vec_pretty(&reply).unwrap();
        if let Some(path) = args.get(4) {
            let _ = storage::atomic_write(std::path::Path::new(path), &bytes);
        } else {
            use std::io::Write;
            let _ = std::io::stdout().write_all(&bytes);
        }
        if failed {
            std::process::exit(1)
        }
        return;
    }
    if args.len() >= 5 && args[1] == "--configuration" {
        let path = std::path::Path::new(&args[4]);
        let result = match args[2].as_str() {
            "export" => configuration::export_configuration(&args[3], path),
            "import" => configuration::import_configuration(&args[3], path),
            "apply" => configuration::apply_configuration(&args[3]),
            _ => Err("未知配置操作".into()),
        };
        if result.is_err() {
            std::process::exit(1)
        }
        return;
    }
    if args.len() >= 4 && (args[1] == "--controls" || args[1] == "--apply-controls") {
        let result = (|| -> Result<(), String> {
            let model = args[2].parse::<usize>().map_err(|e| e.to_string())?;
            if args[1] == "--controls" {
                let state = controls::read(model)?;
                let bytes = serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?;
                storage::atomic_write(std::path::Path::new(&args[3]), &bytes)
            } else {
                let bytes = std::fs::read(&args[3]).map_err(|e| e.to_string())?;
                let action: controls::ControlAction =
                    serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                controls::apply(model, &action).map(|_| ())
            }
        })();
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if args.len() >= 3 && (args[1] == "--export-audio" || args[1] == "--import-audio") {
        let path = std::path::Path::new(&args[2]);
        let result = if args[1] == "--export-audio" {
            profiles::export_audio(path)
        } else {
            profiles::load_audio(path)
                .and_then(|values| audio_fx::apply_profile(&values))
                .map(|_| ())
        };
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    if args.len() >= 3 && (args[1] == "--once" || args[1] == "--diagnose") {
        let result = if args[1] == "--diagnose" {
            configuration::export_diagnostics(None, std::path::Path::new(&args[2]))
                .and_then(|_| std::fs::read(&args[2]).map_err(|e| e.to_string()))
        } else {
            let (snapshot, _) = backend::refresh(&backend::Snapshot::default(), None);
            serde_json::to_vec_pretty(&snapshot).map_err(|e| e.to_string())
        };
        match result.and_then(|bytes| std::fs::write(&args[2], bytes).map_err(|e| e.to_string())) {
            Ok(()) => return,
            Err(e) => {
                ui::message(&format!("导出失败：{e}"));
                std::process::exit(1);
            }
        }
    }
    ui::run(
        args.iter().any(|s| s == "--hidden"),
        args.iter().any(|s| s == "--battery"),
    );
    if let Err(error) = peripheral_desk::resident_macros::shutdown() {
        ui::message(&format!("软件宏停止，但部分按键恢复未完成：{error}。恢复记录已保留，请重新连接设备后在软件宏页面点击停用恢复。"));
    }
}
