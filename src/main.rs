#![windows_subsystem = "windows"]
mod adapters;
mod audio_fx;
mod backend;
mod devices;
mod hid;
mod keyboard;
mod model;
mod paint;
mod profiles;
mod protocol;
mod storage;
mod ui;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--audio-fx-worker") {
        audio_fx::worker_main();
        return;
    }
    let args: Vec<String> = std::env::args().collect();
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
            hid::enumerate().and_then(|v| serde_json::to_vec_pretty(&v).map_err(|e| e.to_string()))
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
}
