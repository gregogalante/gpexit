#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use gpexit_core::{preview, service::Engine};
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tauri::{Manager, State};

#[tauri::command]
async fn api_request(
    engine: State<'_, Arc<Engine>>,
    method: String,
    path: String,
    body: Option<Value>,
) -> Result<Value, String> {
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        engine
            .request(&method, &path, body.unwrap_or(Value::Null))
            .map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn pick_folder(kind: String) -> Result<Option<String>, String> {
    let title = match kind.as_str() {
        "inputs" => "Seleziona cartella ZIP inputs",
        "gallery" => "Seleziona cartella gallery",
        "exports" => "Seleziona cartella exports",
        _ => return Err("Unknown folder type".into()),
    };
    Ok(rfd::AsyncFileDialog::new()
        .set_title(title)
        .pick_folder()
        .await
        .map(|file| file.path().to_string_lossy().into_owned()))
}

#[tauri::command]
async fn media_source(
    app: tauri::AppHandle,
    engine: State<'_, Arc<Engine>>,
    path: String,
) -> Result<Value, String> {
    let engine = engine.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        preview::source(&engine, &path).map_err(|e| format!("{e:#}"))
    })
    .await
    .map_err(|e| e.to_string())??;
    if let Some(path) = result["path"].as_str() {
        app.asset_protocol_scope()
            .allow_file(PathBuf::from(path))
            .map_err(|e| e.to_string())?;
    }
    Ok(result)
}

#[tauri::command]
async fn save_report(engine: State<'_, Arc<Engine>>) -> Result<Option<String>, String> {
    if engine.tasks.running() {
        return Err("Attendere la fine del task.".into());
    }
    let destination = rfd::AsyncFileDialog::new()
        .set_title("Esporta conflitti date")
        .add_filter("CSV", &["csv"])
        .set_file_name("date-conflicts.csv")
        .save_file()
        .await;
    let Some(file) = destination else {
        return Ok(None);
    };
    let path = file.path().to_owned();
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        preview::validate_report_destination(&path).map_err(|e| e.to_string())?;
        engine.save_report(&path).map_err(|e| format!("{e:#}"))?;
        Ok(Some(path.to_string_lossy().into_owned()))
    })
    .await
    .map_err(|e| e.to_string())?
}

fn main() {
    let closing = Arc::new(AtomicBool::new(false));
    let app = tauri::Builder::default()
        .setup(|app| {
            let config_dir = std::env::var_os("GPEXIT_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or(app.path().app_config_dir()?);
            let cache_dir = std::env::var_os("GPEXIT_CACHE_DIR")
                .map(PathBuf::from)
                .unwrap_or(app.path().app_cache_dir()?);
            let settings = config_dir.join("folders.json");
            let cache = cache_dir.join("previews");
            preview::prune_cache(&cache, 512 * 1024 * 1024)?;
            app.manage(Engine::new(settings, cache)?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            api_request,
            pick_folder,
            media_source,
            save_report
        ])
        .build(tauri::generate_context!())
        .expect("Cannot start GPExit desktop");
    app.run(move |app, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            let engine = app.state::<Arc<Engine>>();
            if engine.tasks.running() {
                api.prevent_exit();
                engine.tasks.cancel();
                if !closing.swap(true, Ordering::SeqCst) {
                    let engine = engine.inner().clone();
                    let app = app.clone();
                    std::thread::spawn(move || {
                        engine.tasks.wait();
                        app.exit(0);
                    });
                }
            }
        }
    });
}
