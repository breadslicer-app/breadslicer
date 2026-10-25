//! `#[tauri::command]` as a no-op, so DragonFruit's command files compile unchanged.
use proc_macro::TokenStream;
#[proc_macro_attribute]
pub fn command(_attr: TokenStream, item: TokenStream) -> TokenStream { item }
