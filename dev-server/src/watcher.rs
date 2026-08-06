use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{Mutex, mpsc};
use tracing::{debug, error, info, warn};

use crate::{
    compiler::extract_imports,
    errors::{DevResult, DevServerError},
    hmr::{HmrHub, HmrMessage},
};

// 50 ms is short enough to feel instant, long enough to batch inotify events from a single save.
const DEBOUNCE_MS: u64 = 50;

// Stores reverse edges (dependency → importers) so invalidate() can walk upward.
pub struct DependencyGraph {
    dependents: HashMap<PathBuf, HashSet<PathBuf>>,
    dependencies: HashMap<PathBuf, HashSet<PathBuf>>,
}

impl DependencyGraph {
    pub fn build(theme_dir: &Path) -> DevResult<Self> {
        let canonical = theme_dir.canonicalize().map_err(|e| DevServerError::Io {
            path: theme_dir.display().to_string(),
            source: e,
        })?;

        let mut graph = Self {
            dependents: HashMap::new(),
            dependencies: HashMap::new(),
        };

        let slurp_files = collect_slurp_files(&canonical)?;
        info!(count = slurp_files.len(), "Building dependency graph");

        for file in &slurp_files {
            if let Err(e) = graph.index_file(file) {
                warn!(file = %file.display(), error = %e, "Skipping file during graph build");
            }
        }

        Ok(graph)
    }

    fn index_file(&mut self, file: &Path) -> DevResult<()> {
        let text = std::fs::read_to_string(file).map_err(|e| DevServerError::Io {
            path: file.display().to_string(),
            source: e,
        })?;

        let imports = extract_imports(file, &text)?;

        if let Some(old_deps) = self.dependencies.remove(file) {
            for dep in old_deps {
                if let Some(set) = self.dependents.get_mut(&dep) {
                    set.remove(file);
                }
            }
        }

        let dep_set: HashSet<PathBuf> = imports.iter().cloned().collect();
        for dep in &imports {
            self.dependents
                .entry(dep.clone())
                .or_default()
                .insert(file.to_path_buf());
        }
        self.dependencies.insert(file.to_path_buf(), dep_set);

        debug!(
            file = %file.display(),
            imports = imports.len(),
            "Indexed file"
        );

        Ok(())
    }

    pub fn invalidate(&mut self, changed: &Path) -> Vec<PathBuf> {
        if changed.exists() {
            if let Err(e) = self.index_file(changed) {
                warn!(
                    file = %changed.display(),
                    error = %e,
                    "Failed to re-index changed file"
                );
            }
        } else {
            if let Some(old_deps) = self.dependencies.remove(changed) {
                for dep in old_deps {
                    if let Some(set) = self.dependents.get_mut(&dep) {
                        set.remove(changed);
                    }
                }
            }
            self.dependents.remove(changed);
        }

        let mut visited: HashSet<PathBuf> = HashSet::new();
        let mut queue: Vec<PathBuf> = vec![changed.to_path_buf()];

        while let Some(current) = queue.pop() {
            if !visited.insert(current.clone()) {
                continue;
            }
            if let Some(importers) = self.dependents.get(&current) {
                for importer in importers {
                    if !visited.contains(importer) {
                        queue.push(importer.clone());
                    }
                }
            }
        }

        let affected: Vec<PathBuf> = visited.into_iter().collect();
        debug!(
            changed = %changed.display(),
            affected = affected.len(),
            "Dependency graph invalidated"
        );
        affected
    }
}

type SharedGraph = Arc<Mutex<DependencyGraph>>;

/// Watch `theme_dir` and push an HMR message for every page a change affects.
///
/// The returned handle owns the rebuild task; the dependency graph stays private to it, because
/// nothing outside this module has a reason to read a cache of who imports whom.
pub async fn spawn_watcher(
    theme_dir: PathBuf,
    hub: HmrHub,
) -> DevResult<tokio::task::JoinHandle<()>> {
    let graph = DependencyGraph::build(&theme_dir)?;
    let shared = Arc::new(Mutex::new(graph));

    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
    let shared_clone = Arc::clone(&shared);
    let hub_clone = hub.clone();
    let theme_dir_clone = theme_dir.clone();

    let handle = tokio::spawn(async move {
        let mut pending: HashSet<PathBuf> = HashSet::new();
        let debounce = Duration::from_millis(DEBOUNCE_MS);

        loop {
            match tokio::time::timeout(debounce, rx.recv()).await {
                Ok(Some(path)) => {
                    pending.insert(path);
                    while let Ok(p) = rx.try_recv() {
                        pending.insert(p);
                    }
                }
                Ok(None) => {
                    info!("File watcher channel closed; stopping rebuild task");
                    break;
                }
                Err(_timeout) => {
                    if pending.is_empty() {
                        continue;
                    }
                }
            }

            if pending.is_empty() {
                continue;
            }

            let paths: Vec<PathBuf> = pending.drain().collect();
            process_changes(&paths, &shared_clone, &hub_clone, &theme_dir_clone).await;
        }
    });

    // notify watcher runs on a blocking thread to avoid blocking the async runtime.
    let tx_clone = tx.clone();
    let theme_dir_for_watcher = theme_dir.clone();
    tokio::task::spawn_blocking(move || {
        run_notify_watcher(theme_dir_for_watcher, tx_clone);
    });

    Ok(handle)
}

/// Watch `source_dir` and rebuild it into `theme_dir` whenever a `.slurp` file changes, then tell
/// every connected browser to reload.
///
/// Deliberately built out of the same two pieces as [`spawn_watcher`]: a blocking notify thread
/// that only forwards paths, and an async task that does the work. Setup failures here (a
/// `--source-dir` that does not exist, an exhausted inotify budget) reach `run_notify_watcher`,
/// which logs and returns. A watcher that dies silently is worse than one that never started: the
/// server keeps serving, so the only symptom is edits that stop taking effect.
pub fn spawn_source_watcher(
    slurp_bin: Arc<PathBuf>,
    source_dir: Arc<PathBuf>,
    theme_dir: Arc<PathBuf>,
    globals: Option<Arc<PathBuf>>,
    hub: HmrHub,
) {
    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();

    let watched = source_dir.as_path().to_path_buf();
    tokio::task::spawn_blocking(move || run_notify_watcher(watched, tx));

    tokio::spawn(async move {
        let debounce = Duration::from_millis(DEBOUNCE_MS);
        let mut pending = false;

        loop {
            match tokio::time::timeout(debounce, rx.recv()).await {
                Ok(Some(path)) => {
                    // Rebuild only once the burst has gone quiet. Unlike the theme watcher, which
                    // rechecks individual files, this shells out to a full `slurp build`, so acting
                    // per event would queue N whole-tree builds behind a single save.
                    pending |= path.extension().is_some_and(|e| e == "slurp");
                    continue;
                }
                Ok(None) => {
                    info!("Source watcher channel closed; stopping rebuild task");
                    break;
                }
                Err(_timeout) => {}
            }

            if !pending {
                continue;
            }
            pending = false;

            if let Err(e) = build_source(
                &slurp_bin,
                &source_dir,
                &theme_dir,
                globals.as_deref().map(|p| p.as_path()),
            )
            .await
            {
                error!("{e}");
            }
            hub.broadcast(HmrMessage::Reload {
                pages: vec!["*".to_string()],
            })
            .await;
        }
    });
}

/// Compile `source_dir` into `theme_dir` by invoking the sibling `slurp` CLI.
///
/// The two directories are passed as `OsStr`, not `&str`: a path that is not valid UTF-8 has to
/// reach the child process as itself. Substituting a placeholder would build some other directory
/// and report success.
/// `globals` is forwarded to `slurp build --globals`. Without it the statically built pages lose
/// every `${ platform.* }` and `${ storefront.* }` while dynamic SSR routes keep them, because the
/// SSR path seeds its context from the parsed file directly. That split is invisible: the page
/// renders, and only `<script src="">` and an empty `<title>` say anything is wrong.
pub async fn build_source(
    bin: &Path,
    source_dir: &Path,
    theme_dir: &Path,
    globals: Option<&Path>,
) -> DevResult<()> {
    info!(
        "Building {} -> {}",
        source_dir.display(),
        theme_dir.display()
    );
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("build")
        .arg("--input")
        .arg(source_dir)
        .arg("--output")
        .arg(theme_dir);
    if let Some(globals) = globals {
        cmd.arg("--globals").arg(globals);
    }
    let status = cmd
        .status()
        .await
        .map_err(|e| DevServerError::Compiler(format!("failed to spawn slurp build: {e}")))?;

    if status.success() {
        info!("Build complete");
    } else {
        error!("slurp build failed (exit {status})");
    }
    Ok(())
}

fn run_notify_watcher(theme_dir: PathBuf, tx: mpsc::UnboundedSender<PathBuf>) {
    let tx_inner = tx.clone();

    let handler = move |result: Result<Event, notify::Error>| match result {
        Ok(event) => {
            let is_relevant = matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            );
            if is_relevant {
                for path in event.paths {
                    if is_watched_extension(&path) && tx_inner.send(path).is_err() {
                        return;
                    }
                }
            }
        }
        Err(e) => {
            error!("File watcher error: {e}");
        }
    };

    let mut watcher = match RecommendedWatcher::new(handler, notify::Config::default()) {
        Ok(w) => w,
        Err(e) => {
            error!("Failed to create file watcher: {e}");
            return;
        }
    };

    if let Err(e) = watcher.watch(&theme_dir, RecursiveMode::Recursive) {
        error!(dir = %theme_dir.display(), "Failed to watch theme directory: {e}");
        return;
    }

    info!(dir = %theme_dir.display(), "File watcher active");

    // Park this thread to keep the watcher alive; the handler returns on channel close.
    std::thread::park();
}

fn is_watched_extension(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("slurp" | "css" | "js" | "ts" | "json" | "png" | "jpg" | "jpeg" | "svg" | "webp")
            | None
    )
}

async fn process_changes(
    changed_paths: &[PathBuf],
    shared: &SharedGraph,
    hub: &HmrHub,
    _theme_dir: &Path,
) {
    let mut all_affected: HashSet<PathBuf> = HashSet::new();
    {
        let mut graph = shared.lock().await;
        for path in changed_paths {
            let affected = graph.invalidate(path);
            all_affected.extend(affected);
        }
    }

    if all_affected.is_empty() {
        return;
    }

    info!(
        changed = changed_paths.len(),
        affected = all_affected.len(),
        "Rebuilding pages"
    );

    let mut rebuilt_pages: Vec<String> = Vec::new();
    let mut errors: Vec<crate::errors::SlurpError> = Vec::new();

    for path in &all_affected {
        if path.extension().and_then(|e| e.to_str()) != Some("slurp") {
            // Non-.slurp files (CSS, images) trigger a full reload.
            rebuilt_pages.push("*".to_string());
            continue;
        }

        match crate::compiler::check_page(path).await {
            Ok(diagnostics) => {
                if !diagnostics.is_empty() {
                    warn!(
                        file = %path.display(),
                        count = diagnostics.len(),
                        "Parse diagnostics"
                    );
                    errors.extend(diagnostics);
                }
                rebuilt_pages.push(path_to_url(path, _theme_dir));
            }
            Err(e) => {
                warn!(file = %path.display(), error = %e, "Could not read changed file");
                errors.push(e.to_slurp_error());
            }
        }
    }

    rebuilt_pages.sort();
    rebuilt_pages.dedup();

    if errors.is_empty() {
        // Retract any overlay a previous failing pass put up. Without this, `Clear` had no sender
        // at all and the only thing that ever removed the overlay was the full page reload the
        // browser does on `Reload`. So fixing the error cleared it ONLY when the rebuild also
        // produced a page: edit a component that nothing imports yet, or a file whose rebuild
        // yields no pages, and a stale overlay covers a document that now compiles.
        //
        // Sent unconditionally rather than tracked against the last message, because
        // `removeErrorOverlay` is a no-op when there is no overlay and the hub has no per-client
        // state. A client that connected after the failure is corrected by this too, which a
        // "only if we previously sent Error" version would miss.
        hub.broadcast(HmrMessage::Clear).await;
    } else {
        hub.broadcast(HmrMessage::Error { errors }).await;
    }

    if !rebuilt_pages.is_empty() {
        hub.broadcast(HmrMessage::Reload {
            pages: rebuilt_pages,
        })
        .await;
    }
}

fn path_to_url(path: &Path, theme_dir: &Path) -> String {
    let relative = path.strip_prefix(theme_dir).unwrap_or(path);
    let without_ext = relative.with_extension("");
    let url_path = without_ext
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");

    format!("/{url_path}")
}

fn collect_slurp_files(dir: &Path) -> DevResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_slurp_recursive(dir, &mut files)?;
    Ok(files)
}

fn collect_slurp_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> DevResult<()> {
    let entries = std::fs::read_dir(dir).map_err(|e| DevServerError::Io {
        path: dir.display().to_string(),
        source: e,
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| DevServerError::Io {
            path: dir.display().to_string(),
            source: e,
        })?;

        let path = entry.path();
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            collect_slurp_recursive(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("slurp") {
            out.push(path);
        }
    }

    Ok(())
}
