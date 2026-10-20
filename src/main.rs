#![windows_subsystem = "windows"]
mod backend;
mod hid;
mod keyboard;
mod protocol;
mod ui;

fn main() {
    let args: Vec<String> = std::env::args().collect();
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
    ui::run(args.iter().any(|s| s == "--hidden"));
}
