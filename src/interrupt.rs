//! Ctrl-C during a foreground delivery (`kiln start`, `kiln resume`) becomes a
//! pause request through the run control mechanism instead of killing Kiln.
//!
//! The signal handler only counts interrupts (async-signal-safe). A watcher
//! thread reports them on the terminal and, while the scheduler or publication
//! owns the run, requests a pause through [`Engine::request_control`], so the
//! run stops at its next safe point. Between those phases the main thread checks
//! [`requested`] itself and records the pause. Further Ctrl-C presses are only
//! acknowledged: the handler stays installed, so they never cut a write short.
use kiln::Engine;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

static INTERRUPTS: AtomicUsize = AtomicUsize::new(0);
static RUN: Mutex<Option<String>> = Mutex::new(None);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTS.fetch_add(1, Ordering::SeqCst);
}

/// Turn SIGINT into pause requests for the rest of the process.
pub fn install(engine: &Engine) {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
    }
    let engine = engine.clone();
    std::thread::spawn(move || watch_loop(&engine));
}

/// Record the run that Ctrl-C should pause.
pub fn watch(id: &str) {
    *RUN.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_owned());
}

/// True once Ctrl-C was pressed.
pub fn requested() -> bool {
    INTERRUPTS.load(Ordering::SeqCst) > 0
}

fn watch_loop(engine: &Engine) {
    let mut seen = 0;
    loop {
        let count = INTERRUPTS.load(Ordering::SeqCst);
        let run = RUN.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if count > seen {
            let which = run
                .as_deref()
                .map(|id| format!(" run {id}"))
                .unwrap_or_default();
            if seen == 0 {
                eprintln!(
                    "Pausing{which}: waiting for active work to reach a safe point. Run `kiln resume` to continue it later."
                );
            } else {
                eprintln!(
                    "Still pausing{which} at the next safe point; nothing is interrupted half-way."
                );
            }
            seen = count;
        }
        if count > 0 {
            if let Some(id) = &run {
                // Only an owned, running run takes a control request; retry so a
                // scheduler-to-publication handover cannot drop the pause.
                if engine.inspect(id).is_ok_and(|run| run.status == "running") {
                    let _ = engine.request_control(id, "pause");
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
