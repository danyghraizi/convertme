mod conversion;

use serde::Serialize;
use std::path::PathBuf;
use tauri_plugin_dialog::DialogExt;

#[derive(Debug, Serialize)]
struct PickedFile {
    path: String,
    name: String,
}

#[tauri::command]
fn pick_images(app: tauri::AppHandle) -> Vec<PickedFile> {
    app.dialog()
        .file()
        .add_filter(
            "Images",
            &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tif", "tiff"],
        )
        .blocking_pick_files()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|file| file.into_path().ok())
        .map(|path| PickedFile {
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            path: path.to_string_lossy().into_owned(),
        })
        .collect()
}

#[tauri::command]
fn convert_images(
    paths: Vec<String>,
    format: String,
    max_width: Option<u32>,
    max_height: Option<u32>,
    rotation: u16,
    jpeg_quality: u8,
) -> conversion::BatchResult {
    let paths = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
    let options = conversion::ConvertOptions {
        format,
        max_width,
        max_height,
        rotation,
        jpeg_quality,
    };
    conversion::convert_batch(&paths, &options)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![pick_images, convert_images])
        .run(tauri::generate_context!())
        .expect("failed to run ConvertMe Image");
}
