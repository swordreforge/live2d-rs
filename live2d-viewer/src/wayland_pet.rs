use std::path::PathBuf;
use std::sync::mpsc;

/// Main thread → Pet thread commands
pub enum PetCommand {
    Enter {
        model_dir: PathBuf,
        model_format: crate::app::ModelFormat,
    },
    Exit,
}

/// Pet thread → Main thread events
pub enum PetEvent {
    Configured { width: u32, height: u32 },
    Error(String),
    Exited,
}

/// Spawn a separate thread that creates an sctk layer-shell surface + GL context.
///
/// Returns a `JoinHandle` for the pet thread.
/// The caller sends commands via `cmd_tx` to control the thread lifecycle.
pub fn spawn_pet_surface(
    cmd_rx: mpsc::Receiver<PetCommand>,
    _event_tx: mpsc::Sender<PetEvent>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        log::info!("[pet/wayland] thread started");
        // Wait for Enter command
        loop {
            match cmd_rx.recv() {
                Ok(PetCommand::Enter {
                    model_dir,
                    model_format: _,
                }) => {
                    log::info!("[pet/wayland] enter: {:?}", model_dir);
                    // Tasks 3–5 will expand here
                    break;
                }
                Ok(PetCommand::Exit) | Err(_) => {
                    log::info!("[pet/wayland] exited before enter");
                    return;
                }
            }
        }
        // Event + render loop (Tasks 3–5)
        log::info!("[pet/wayland] thread ended");
    })
}
