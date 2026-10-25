//! The slice of Tauri DragonFruit's island commands touch: the command
//! attribute, `spawn_blocking` (run inline) and event emission (dropped).
pub use tauri_macros_stub::command;
pub mod async_runtime {
    pub fn spawn_blocking<T, F: FnOnce() -> T>(f: F) -> std::future::Ready<Result<T, String>> {
        std::future::ready(Ok(f()))
    }
}
#[derive(Clone, Default)]
pub struct AppHandle;
pub trait Emitter {
    fn emit<S: serde::Serialize>(&self, _event: &str, _payload: S) -> Result<(), String> { Ok(()) }
}
impl Emitter for AppHandle {}
