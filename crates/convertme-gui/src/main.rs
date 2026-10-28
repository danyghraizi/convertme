//! One binary, two front ends: `convertme <subcommand>` is the CLI, and no
//! arguments (or a dropped file) opens the window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let is_cli = args
        .first()
        .map(|a| {
            matches!(
                a.as_str(),
                "convert"
                    | "tool"
                    | "probe"
                    | "history"
                    | "sweep"
                    | "help"
                    | "--help"
                    | "-h"
                    | "--version"
                    | "-V"
                    | "version"
            )
        })
        .unwrap_or(false);

    let code = if is_cli {
        convertme_gui::cli::run(&args)
    } else {
        convertme_gui::app::run()
    };
    std::process::exit(code);
}
