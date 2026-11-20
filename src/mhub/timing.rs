//! Optional local acceptance trace. Only operation labels and elapsed time are recorded.
use std::{cell::RefCell, io::Write, time::Instant};

struct Trace {
    label: String,
    origin: Instant,
    frame: bool,
    editable: bool,
}

thread_local! { static TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) }; }

pub fn start(label: &str) {
    if std::env::var_os("PD_HUB_TIMING_FILE").is_none() {
        return;
    }
    TRACE.set(Some(Trace {
        label: label.into(),
        origin: Instant::now(),
        frame: false,
        editable: false,
    }));
    mark("click");
}

pub fn mark(stage: &'static str) {
    TRACE.with_borrow_mut(|trace| {
        let Some(t) = trace else { return };
        match stage {
            "first_frame" if std::mem::replace(&mut t.frame, true) => return,
            "editable" if std::mem::replace(&mut t.editable, true) => return,
            _ => {}
        }
        if let Some(path) = std::env::var_os("PD_HUB_TIMING_FILE") {
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(file, "{}", serde_json::json!({"entry": t.label, "stage": stage, "elapsed_ms": t.origin.elapsed().as_secs_f64()*1000.0}));
            }
        }
    });
}
