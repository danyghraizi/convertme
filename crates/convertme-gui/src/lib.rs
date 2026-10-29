//! `convertme-gui`: the Tauri shell, the wheel window, the actions and the CLI.
//!
//! The window is deliberately thin. Everything that decides *what* to do lives in
//! `convertme-core`, everything that *does* it lives in `convertme-engines`, and
//! everything platform-specific about observing input lives in `convertme-drag`.
//!
//! `logging` is declared first and with `#[macro_use]` so `logln!` is available to
//! every module below it.

#[macro_use]
pub mod logging;

pub mod actions;
pub mod app;
pub mod autostart;
pub mod cli;
pub mod commands;
pub mod instance;
pub mod overlay;
pub mod state;

pub use app::run;

#[cfg(test)]
mod embedded_bundle {
    /// The window shows whatever was embedded when this crate was last compiled, so a
    /// frontend change that never made it into the binary looks like a UI that ignored
    /// an edit. Compare the embedded page with the built one.
    #[test]
    fn the_embedded_page_is_the_built_one() {
        use tauri::utils::assets::AssetKey;
        let context: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let asset = context
            .assets()
            .get(&AssetKey::from("index.html"))
            .expect("index.html is embedded");
        // Tauri normalises the markup, so compare the hashed bundle it points at: that
        // name changes with every frontend build.
        let script_of = |bytes: &[u8]| -> Option<String> {
            let text = String::from_utf8_lossy(bytes);
            let start = text.find("assets/index-")?;
            let rest = &text[start..];
            let end = rest.find('"')?;
            Some(rest[..end].to_string())
        };
        let embedded = script_of(asset.as_ref()).expect("the embedded page names a bundle");
        let built_bytes = std::fs::read("ui-dist/index.html").expect("ui-dist/index.html");
        let built = script_of(&built_bytes).expect("the built page names a bundle");
        assert_eq!(
            embedded, built,
            "the binary embeds an older page: rebuild the frontend and this crate"
        );

        // The bundle itself, not just its name: this is what the window executes.
        let script = context
            .assets()
            .get(&AssetKey::from(embedded.as_str()))
            .expect("the embedded bundle exists");
        let code = String::from_utf8_lossy(script.as_ref());
        assert!(
            code.contains("colour-trigger") && code.contains("gradient-strip"),
            "the embedded bundle predates the colour picker and the gradient editor"
        );
    }
}
