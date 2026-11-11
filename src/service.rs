use crate::{
    archive,
    config::{self, Folders},
    files, gallery,
    task::TaskManager,
};
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, TryLockError},
};
use url::Url;

type Snapshot = Option<Arc<Vec<Value>>>;

pub struct Engine {
    folders: Mutex<Option<Folders>>,
    pub tasks: TaskManager,
    gate: Mutex<()>,
    settings_path: PathBuf,
    pub cache_path: PathBuf,
    gallery_lock: Mutex<Option<(PathBuf, File)>>,
    gallery_cache: Mutex<Snapshot>,
    conflict_cache: Mutex<Snapshot>,
    pub preview_lock: Mutex<()>,
    startup_error: Mutex<Option<String>>,
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>> {
    mutex
        .lock()
        .map_err(|_| anyhow::anyhow!("Application state unavailable; restart GPExit."))
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a str> {
    value
        .get(name)
        .and_then(Value::as_str)
        .with_context(|| format!("Missing {name}"))
}

impl Engine {
    pub fn new(settings_path: PathBuf, cache_path: PathBuf) -> Result<Arc<Self>> {
        fs::create_dir_all(&cache_path)?;
        let (folders, error) = match config::load(&settings_path) {
            Ok(folders) => (folders, None),
            Err(e) => (None, Some(format!("Cannot read settings: {e:#}"))),
        };
        let engine = Arc::new(Self {
            folders: Mutex::new(folders),
            tasks: TaskManager::default(),
            gate: Mutex::new(()),
            settings_path,
            cache_path,
            gallery_lock: Mutex::new(None),
            gallery_cache: Mutex::new(None),
            conflict_cache: Mutex::new(None),
            preview_lock: Mutex::new(()),
            startup_error: Mutex::new(error),
        });
        let saved = lock(&engine.folders)?.clone();
        if let Some(saved) = saved {
            match saved.validate().and_then(|folders| {
                engine.acquire_gallery(&folders)?;
                Ok(folders)
            }) {
                Ok(folders) => *lock(&engine.folders)? = Some(folders),
                Err(error) => *lock(&engine.startup_error)? = Some(format!("{error:#}")),
            }
        }
        Ok(engine)
    }
    fn open_gallery_lock(folders: &Folders) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(folders.gallery.join(".gpexit-desktop.lock"))?;
        file.try_lock_exclusive()
            .context("Gallery already open in another GPExit desktop instance")?;
        Ok(file)
    }
    fn acquire_gallery(&self, folders: &Folders) -> Result<()> {
        let mut held = lock(&self.gallery_lock)?;
        if held.as_ref().is_some_and(|(p, _)| *p == folders.gallery) {
            return Ok(());
        }
        let file = Self::open_gallery_lock(folders)?;
        *held = Some((folders.gallery.clone(), file));
        Ok(())
    }
    pub fn configured_folders(&self) -> Result<Folders> {
        if let Some(error) = &*lock(&self.startup_error)? {
            bail!("{error}");
        }
        lock(&self.folders)?
            .clone()
            .context("Select inputs, gallery and exports in Settings first.")
    }
    pub fn invalidate(&self) {
        if let Ok(mut cache) = self.gallery_cache.lock() {
            *cache = None;
        }
        if let Ok(mut cache) = self.conflict_cache.lock() {
            *cache = None;
        }
    }
    fn settings(&self) -> Result<Value> {
        let folders = lock(&self.folders)?.clone();
        let configured = self.configured_folders().is_ok();
        let error = lock(&self.startup_error)?.clone();
        Ok(json!({"folders":folders, "configured":configured, "error":error}))
    }
    fn idle_guard(&self) -> Result<MutexGuard<'_, ()>> {
        let guard = match self.gate.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => bail!("Attendere la fine dell’operazione in corso."),
            Err(_) => bail!("Application state unavailable; restart GPExit."),
        };
        if self.tasks.running() {
            bail!("Attendere la fine del task.");
        }
        Ok(guard)
    }
    fn snapshot(
        &self,
        folders: &Folders,
        conflicts: bool,
        refresh: bool,
    ) -> Result<Arc<Vec<Value>>> {
        let mut cache = lock(if conflicts {
            &self.conflict_cache
        } else {
            &self.gallery_cache
        })?;
        if refresh || cache.is_none() {
            *cache = Some(Arc::new(if conflicts {
                gallery::conflicts(folders)?
            } else {
                gallery::list_gallery(folders)?
            }));
        }
        Ok(cache.as_ref().unwrap().clone())
    }
    pub fn request(self: &Arc<Self>, method: &str, path: &str, body: Value) -> Result<Value> {
        let url = Url::parse(&format!("http://local{path}"))?;
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        let text = |key: &str| query.get(key).map(String::as_str).unwrap_or("");
        let page = query
            .get("page")
            .map(|p| p.parse::<usize>())
            .transpose()?
            .unwrap_or(1)
            .max(1);
        if method == "GET" && url.path() == "/api/settings" {
            return self.settings();
        }
        if method == "GET" && url.path() == "/api/state" {
            let folders = lock(&self.folders)?.clone();
            let valid = self.configured_folders().is_ok();
            return Ok(
                json!({"configured":valid, "folders":folders, "error":*lock(&self.startup_error)?,
                "gallery":folders.as_ref().map(|f| f.gallery.to_string_lossy().to_string()).unwrap_or_default(),
                "inputs":if valid { listing(&folders.as_ref().unwrap().inputs)? } else { vec![] },
                "exports":if valid { listing(&folders.as_ref().unwrap().exports)? } else { vec![] },
                "task":self.tasks.status()}),
            );
        }
        if method == "POST" && url.path() == "/api/cancel" {
            self.tasks.cancel();
            return Ok(json!({"ok":true}));
        }
        if method == "POST" && url.path() == "/api/settings" {
            let _guard = self.idle_guard()?;
            let folders: Folders = serde_json::from_value(body)?;
            let folders = folders.validate()?;
            // Retain the previous gallery lock if persisting the replacement settings fails.
            let mut held = lock(&self.gallery_lock)?;
            let replacement = if held
                .as_ref()
                .is_some_and(|(path, _)| *path == folders.gallery)
            {
                None
            } else {
                Some(Self::open_gallery_lock(&folders)?)
            };
            config::save(&self.settings_path, &folders)?;
            if let Some(file) = replacement {
                *held = Some((folders.gallery.clone(), file));
            }
            drop(held);
            *lock(&self.folders)? = Some(folders);
            *lock(&self.startup_error)? = None;
            self.invalidate();
            return self.settings();
        }
        let folders = self.configured_folders()?;
        if method == "POST" && url.path() == "/api/tasks" {
            let _guard = self.idle_guard()?;
            let action = field(&body, "action")?.to_owned();
            let password = body
                .get("password")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let workers = body.get("workers").and_then(Value::as_u64).unwrap_or(4);
            if !(1..=4).contains(&workers) {
                bail!("Workers must be between 1 and 4.");
            }
            let paths = if action == "import" {
                let names = body
                    .get("inputs")
                    .and_then(Value::as_array)
                    .context("Select input ZIP files")?;
                if names.is_empty() {
                    bail!("Select at least one ZIP file.");
                }
                let allowed = listing(&folders.inputs)?;
                let mut paths = vec![];
                for name in names {
                    let name = name.as_str().context("Invalid ZIP name")?;
                    if !allowed.iter().any(|file| file["name"] == name) {
                        bail!("Input ZIP not found: {name}");
                    }
                    paths.push(folders.inputs.join(name));
                }
                paths
            } else if action == "export" {
                if password
                    != body
                        .get("password_confirmation")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                {
                    bail!("Le password di esportazione non coincidono.");
                }
                vec![]
            } else {
                bail!("Unknown task");
            };
            let engine = self.clone();
            self.tasks.start(move |context| {
                let _guard = lock(&engine.gate)?;
                let result = if action == "import" {
                    archive::import_archives(
                        &folders,
                        &paths,
                        &password,
                        workers as usize,
                        &context,
                    )
                } else {
                    archive::export_archives(&folders, &password, &context)
                };
                engine.invalidate();
                result
            })?;
            return Ok(json!({"ok":true}));
        }
        if method == "GET" && url.path() == "/api/storage" {
            return gallery::storage_stats(&folders);
        }
        let _guard = self.idle_guard()?;
        match (method, url.path()) {
            ("GET", "/api/gallery" | "/api/gallery/options") => {
                let items = self.snapshot(&folders, false, text("refresh") == "1")?;
                if url.path().ends_with("/options") {
                    let years: std::collections::BTreeSet<_> =
                        items.iter().filter_map(|v| v["year"].as_str()).collect();
                    let months: std::collections::BTreeSet<_> =
                        items.iter().filter_map(|v| v["month"].as_str()).collect();
                    return Ok(json!({"years":years,"months":months}));
                }
                let review = text("review");
                if !["", "pending", "all"].contains(&review) {
                    bail!("Invalid review filter");
                }
                let selected: Vec<_> = items
                    .iter()
                    .filter(|v| {
                        (review == "all" || v["kept"] != true)
                            && (text("year").is_empty() || v["year"] == text("year"))
                            && (text("month").is_empty() || v["month"] == text("month"))
                    })
                    .collect();
                Ok(paginate(&selected, page))
            }
            ("GET", "/api/conflicts") => Ok(paginate(
                self.snapshot(&folders, true, text("refresh") == "1")?
                    .as_ref(),
                page,
            )),
            ("GET", "/api/trash") => Ok(paginate(&gallery::list_trash(&folders)?, page)),
            ("POST", "/api/gallery/keep") => {
                gallery::keep_file(&folders, field(&body, "path")?)?;
                self.invalidate();
                Ok(json!({"ok":true}))
            }
            ("POST", "/api/gallery/trash") => {
                let id = gallery::trash_file(&folders, field(&body, "path")?)?;
                self.invalidate();
                Ok(json!({"id":id}))
            }
            ("POST", "/api/gallery/restore") => {
                let path = gallery::restore_file(&folders, field(&body, "id")?)?;
                self.invalidate();
                Ok(json!({"path":path}))
            }
            ("POST", "/api/correct" | "/api/keep") => {
                let relative = field(&body, "path")?;
                let date = if url.path() == "/api/keep" {
                    let path = files::media_path(&folders.gallery, relative)?;
                    let metadata: Value =
                        serde_json::from_slice(&fs::read(files::sidecar(&path))?)?;
                    crate::dates::google_date(&metadata)
                        .context("Google date unavailable")?
                        .to_string()
                } else {
                    field(&body, "date")?.into()
                };
                let destination = gallery::correct_date(&folders, relative, &date)?;
                self.invalidate();
                Ok(json!({"path":destination}))
            }
            _ => bail!("Unknown operation"),
        }
    }
    pub fn save_report(&self, destination: &Path) -> Result<()> {
        let _guard = self.idle_guard()?;
        let folders = self.configured_folders()?;
        let rows = gallery::conflicts(&folders)?;
        let parent = destination.parent().context("Invalid report destination")?;
        let mut report = tempfile::NamedTempFile::new_in(parent)?;
        use std::io::Write;
        writeln!(report, "file,filename_date,google_date")?;
        for row in rows {
            let values: Vec<_> = ["path", "filename_date", "google_date"]
                .iter()
                .map(|key| {
                    format!(
                        "\"{}\"",
                        row[key].as_str().unwrap_or("").replace('"', "\"\"")
                    )
                })
                .collect();
            writeln!(report, "{}", values.join(","))?;
        }
        report.as_file().sync_all()?;
        report.persist(destination).map_err(|e| e.error)?;
        Ok(())
    }
}

pub fn listing(folder: &Path) -> Result<Vec<Value>> {
    let mut items = vec![];
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_symlink()
            || !entry.file_type()?.is_file()
            || !path
                .extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("zip"))
        {
            continue;
        }
        items.push(
            json!({"name":entry.file_name().to_string_lossy(), "size":entry.metadata()?.len()}),
        );
    }
    items.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(items)
}

pub fn paginate<T: serde::Serialize>(items: &[T], page: usize) -> Value {
    let pages = items.len().div_ceil(24).max(1);
    let page = page.max(1).min(pages);
    let start = (page - 1) * 24;
    json!({"items":&items[start..items.len().min(start+24)],"page":page,"pages":pages,"total":items.len()})
}
