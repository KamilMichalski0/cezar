//! cezar desktop (PoC): a native window around the managed cezar install.
//!
//! The shell is deliberately thin. It resolves the entry file of the ACTIVE managed version
//! (`~/.cezar/versions/current`), runs it as a sidecar through the user's login shell (so the
//! `node`, `claude`, `gh` on their PATH are the ones cezar sees — a GUI app inherits none of
//! that), waits for `/api/v1/health`, and points the webview at the cockpit. Everything the
//! cockpit does — including updating cezar — happens in the sidecar: an update ends with the
//! sidecar exiting `75` (`CEZ_SUPERVISED=1`), and the supervisor loop below relaunches it,
//! which picks up the freshly activated version.
//!
//! The one thing the shell does on its own is the SUPERVISOR's job: put a cezar in place when
//! there is none (first launch) and put the newest one in place on request (the app menu's
//! "Update cezar to latest…"), so a downgrade into a version that predates the cockpit's own
//! updater is never a dead end. That is `npm install --prefix` into the same layout the
//! cockpit's updater uses — no cezar code needed, any version recoverable.
//!
//! No Tauri IPC is exposed to the cockpit beyond window dragging; the splash page is driven
//! with `eval`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

/// Exit status the sidecar uses to say "relaunch me" after a self-update (EX_TEMPFAIL).
const RESTART_EXIT_CODE: i32 = 75;
const HEALTH_TIMEOUT: Duration = Duration::from_secs(60);
const LOG_TAIL: usize = 60;
/// The cockpit's conventional port, tried first so `http://localhost:4321` works in a browser
/// beside the app whenever it can; a busy one falls through to the next few, then to any.
const PREFERRED_PORTS: std::ops::Range<u16> = 4321..4331;
const PACKAGE: &str = "@open-mercato/cezar";

/// Everything the supervisor thread and the menu handler share.
struct Shell {
    child: Mutex<Option<Child>>,
    port: AtomicU16,
    /// Set by the menu's update flow before it kills the sidecar: the supervisor loop treats the
    /// resulting exit as "relaunch" instead of "crashed".
    restart_requested: AtomicBool,
    updating: AtomicBool,
    open_item: Mutex<Option<MenuItem<tauri::Wry>>>,
    /// "Versions" submenu — one check item per managed install, rebuilt whenever the set or the
    /// active one changes. Switching is the recovery path from ANY version, however old.
    versions_menu: Mutex<Option<Submenu<tauri::Wry>>>,
    /// Set on every move/resize; a background writer persists the geometry shortly after.
    geometry_dirty: AtomicBool,
    /// The running sidecar's version (from health) and the channel's newest when it is newer —
    /// what the title strip's "Update cezar" button (legacy cockpits) is driven by.
    running_version: Mutex<Option<String>>,
    update_available: Mutex<Option<String>>,
}

/// Runs at document start on EVERY page the window loads — the splash and, after navigation,
/// the cockpit. The cockpit reads `data-cez-desktop` to paint the title band the traffic lights
/// sit in (see `packages/web/src/components/app-shell.tsx`); a browser tab on the same server
/// never sees it.
const INIT_SCRIPT: &str = r#"
  (function () {
    var platform = "__PLATFORM__";
    window.__CEZ_DESKTOP__ = { platform: platform };
    document.documentElement.dataset.cezDesktop = platform;
    if (platform !== "macos" || location.protocol !== "http:") return;

    // The title strip's "Update cezar" pill, for cockpits that predate the desktop-aware build
    // (those paint their own from health's `latestVersion`). The shell calls `showUpdate` once it
    // has compared the running version with the channel's newest on the registry.
    var pending = null;
    function renderPill(version) {
      if (document.querySelector('[data-cez-update-pill]')) return;
      var strip = document.querySelector('[data-cez-legacy-titlebar]');
      if (!strip) { pending = version; return; }
      var pill = document.createElement('button');
      pill.type = 'button';
      pill.setAttribute('data-cez-update-pill', '');
      pill.title = 'Update cezar to v' + version + ' and restart';
      pill.style.cssText = 'position:fixed;top:5px;left:80px;height:18px;z-index:2147483001;' +
        'display:inline-flex;align-items:center;gap:6px;padding:0 8px;border-radius:999px;' +
        'border:1px solid rgba(168,243,114,.45);background:rgba(168,243,114,.16);color:inherit;' +
        'font:600 11px/1 -apple-system,BlinkMacSystemFont,Inter,system-ui,sans-serif;cursor:pointer;' +
        '-webkit-app-region:no-drag;';
      pill.innerHTML = '<span style="width:5px;height:5px;border-radius:50%;background:#fbbf24;animation:cezPulse 1.6s ease-in-out infinite"></span>' +
        'Update cezar <span style="font-family:ui-monospace,Menlo,monospace;font-weight:500;opacity:.7">v' + version + '</span>';
      pill.onmouseenter = function () { pill.style.background = 'rgba(168,243,114,.3)'; };
      pill.onmouseleave = function () { pill.style.background = 'rgba(168,243,114,.16)'; };
      pill.onclick = function () {
        pill.disabled = true;
        pill.style.opacity = '.6';
        if (window.__TAURI_INTERNALS__) window.__TAURI_INTERNALS__.invoke('update_cezar_command');
      };
      var style = document.createElement('style');
      style.textContent = '@keyframes cezPulse{0%,100%{opacity:1}50%{opacity:.35}}';
      document.head.appendChild(style);
      (document.querySelector('[data-slot="app-shell"]') || document.body).appendChild(pill);
    }
    // `offer_update` (Rust) calls this once the shell knows the channel has something newer.
    window.__CEZ_DESKTOP__.showUpdate = function (version) {
      if (!version) return;
      // A desktop-aware cockpit shows its own pill — never two.
      if (document.querySelector('[data-slot="desktop-titlebar"]')) return;
      renderPill(version);
    };

    // A cockpit that knows about the shell paints its own transparent title strip
    // (`data-slot="desktop-titlebar"`). One that predates it does not, and the traffic lights
    // would land on its brand row — so once the page has rendered, give it a transparent strip
    // and inset its columns 28px, so they paint the band in their own (live) theme colours.
    var attempts = 0;
    var timer = setInterval(function () {
      attempts += 1;
      if (document.querySelector('[data-slot="desktop-titlebar"]')) { clearInterval(timer); return; }
      var shell = document.querySelector('[data-slot="app-shell"]');
      if (!shell) { if (attempts > 40) clearInterval(timer); return; }
      clearInterval(timer);
      // Transparent strip + each column padded 28px: the columns paint the band in their OWN
      // theme colours, so a light/dark switch in the cockpit changes the band with it — a
      // colour read once here would not.
      var strip = document.createElement('div');
      strip.setAttribute('data-tauri-drag-region', '');
      strip.setAttribute('data-cez-legacy-titlebar', '');
      strip.style.cssText = 'position:fixed;top:0;left:0;right:0;height:28px;z-index:2147483000;' +
        '-webkit-user-select:none;user-select:none;';
      shell.appendChild(strip);
      var style = document.createElement('style');
      style.textContent =
        '[data-slot="app-shell"]>aside[data-slot="sidebar"],[data-slot="app-shell"]>div{padding-top:28px!important}' +
        '[data-slot="sidebar-content"]>div:first-child{padding-top:6px!important}';
      document.head.appendChild(style);
      if (pending) { var v = pending; pending = null; renderPill(v); }
    }, 100);
  })();
"#;

fn platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

fn build_main_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("cezar")
        .inner_size(1360.0, 900.0)
        .min_inner_size(720.0, 480.0)
        .center()
        // Hidden until the saved geometry is applied, so the window never flashes at the
        // default size and position before jumping to where the user left it.
        .visible(false)
        .initialization_script(INIT_SCRIPT.replace("__PLATFORM__", platform_name()));
    // macOS: no title text; the traffic lights keep their NATIVE placement (a standard title bar
    // is 28pt tall and puts them at the system offset) and float over the 28px band the cockpit
    // paints at the top (`data-slot="desktop-titlebar"`). Native geometry, not ours — the
    // lights then sit exactly where every other app's do.
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);
    let window = builder.build()?;
    let restored = apply_saved_geometry(&window);
    let _ = window.show();
    // macOS resolves a hidden window's screen lazily; re-assert the position once it is on
    // screen so the first paint lands where it should, not where AppKit cascaded it.
    if let Some((x, y)) = restored {
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    }
    if cfg!(debug_assertions) {
        let probe = window.clone();
        std::thread::spawn(move || {
            for delay in [0u64, 500, 1500, 4000] {
                std::thread::sleep(Duration::from_millis(delay));
                if let (Ok(position), Ok(size), Ok(scale)) = (probe.outer_position(), probe.inner_size(), probe.scale_factor()) {
                    let position = position.to_logical::<f64>(scale);
                    let size = size.to_logical::<f64>(scale);
                    eprintln!("[geometry] +{delay}ms at ({}, {}) {}x{} scale {scale}", position.x, position.y, size.width, size.height);
                }
            }
        });
    }
    Ok(window)
}

fn build_menu(app: &AppHandle, shell: &Shell) -> tauri::Result<()> {
    let update_item = MenuItem::with_id(app, "update-cezar", "Update cezar to latest…", true, None::<&str>)?;
    let open_item = MenuItem::with_id(app, "open-browser", "Open view in browser", true, Some("CmdOrCtrl+Shift+O"))?;
    *shell.open_item.lock().unwrap() = Some(open_item.clone());
    let versions_menu = Submenu::with_id(app, "versions", "Versions", true)?;
    *shell.versions_menu.lock().unwrap() = Some(versions_menu.clone());
    refresh_versions_menu(app, shell);
    let app_menu = Submenu::with_items(
        app,
        "cezar",
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &PredefinedMenuItem::separator(app)?,
            &update_item,
            &versions_menu,
            &open_item,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    // Without an Edit menu the webview has no Cmd+C / Cmd+V on macOS.
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;
    app.set_menu(Menu::with_items(app, &[&app_menu, &edit_menu, &window_menu])?)?;
    Ok(())
}

/// One installed managed version, as the Versions submenu lists it.
struct InstalledVersion {
    id: String,
    label: String,
    active: bool,
}

/// Every complete install under `~/.cezar/versions` (a manifest AND an entry file), newest
/// install first — the same rule as cezar's own `listInstalled`.
fn installed_versions() -> Vec<InstalledVersion> {
    let dir = cezar_home().join("versions");
    let active = std::fs::read_link(dir.join("current"))
        .ok()
        .and_then(|target| target.file_name().map(|name| name.to_string_lossy().into_owned()));
    let mut rows: Vec<(String, InstalledVersion)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if id == "current" || id.starts_with('.') {
                continue;
            }
            let manifest = std::fs::read_to_string(entry.path().join(".cezar-install.json"))
                .ok()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok());
            let Some(manifest) = manifest else { continue };
            if !entry.path().join("node_modules").join("@open-mercato").join("cezar").join("dist").join("index.js").is_file() {
                continue;
            }
            let version = manifest.get("version").and_then(|v| v.as_str()).unwrap_or(&id).to_string();
            let source = manifest.get("source").and_then(|v| v.as_str()).unwrap_or("registry");
            let installed_at = manifest.get("installedAt").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let label = if source == "local" { format!("{version} (local build)") } else { version };
            rows.push((installed_at, InstalledVersion { active: active.as_deref() == Some(id.as_str()), id, label }));
        }
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows.into_iter().map(|(_, row)| row).collect()
}

/// Rebuild the Versions submenu from disk: a check item per install, the active one checked.
fn refresh_versions_menu(app: &AppHandle, shell: &Shell) {
    let guard = shell.versions_menu.lock().unwrap();
    let Some(menu) = guard.as_ref() else { return };
    if let Ok(items) = menu.items() {
        for item in items {
            let _ = menu.remove(&item);
        }
    }
    let versions = installed_versions();
    if versions.is_empty() {
        if let Ok(item) = MenuItem::with_id(app, "versions-none", "No versions installed", false, None::<&str>) {
            let _ = menu.append(&item);
        }
        return;
    }
    for version in versions {
        if let Ok(item) = CheckMenuItem::with_id(app, format!("use-version:{}", version.id), &version.label, !version.active, version.active, None::<&str>) {
            let _ = menu.append(&item);
        }
    }
}

/// Point `current` at an installed id (atomic rename over the old link) and relaunch.
fn switch_version(shell: &Shell, id: &str) -> bool {
    let dir = cezar_home().join("versions");
    if !dir.join(id).join("node_modules").join("@open-mercato").join("cezar").join("dist").join("index.js").is_file() {
        return false;
    }
    let tmp = dir.join(format!(".current.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(id, &tmp).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(id, &tmp).is_ok() || std::fs::write(&tmp, id).is_ok();
    if !linked || std::fs::rename(&tmp, dir.join("current")).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    let mut guard = shell.child.lock().unwrap();
    if let Some(child) = guard.as_mut() {
        shell.restart_requested.store(true, Ordering::SeqCst);
        let _ = child.kill();
    }
    true
}

/// The legacy strip's "Update cezar" button lands here (capability: `allow-update-cezar` for
/// the cockpit's origin). Same flow as the menu item.
#[tauri::command]
fn update_cezar_command(app: AppHandle, shell: tauri::State<'_, Arc<Shell>>) {
    let app = app.clone();
    let shell = shell.inner().clone();
    std::thread::spawn(move || update_cezar(&app, &shell, "Updating cezar…"));
}

/// semver-ish ordering for what cezar publishes (`0.11.1`, `0.11.1-nightly.20260924.49`):
/// numeric core, a release above any prerelease of the same core, then identifiers.
fn version_newer(candidate: &str, current: &str) -> bool {
    fn parse(raw: &str) -> Option<([u64; 3], Vec<String>)> {
        let raw = raw.trim().trim_start_matches('v').split('+').next()?;
        let (core, pre) = match raw.split_once('-') {
            Some((core, pre)) => (core, pre.split('.').map(str::to_owned).collect::<Vec<_>>()),
            None => (raw, Vec::new()),
        };
        let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
        Some(([parts.next()??, parts.next()??, parts.next()??], pre))
    }
    let (Some((a, pa)), Some((b, pb))) = (parse(candidate), parse(current)) else { return false };
    if a != b {
        return a > b;
    }
    match (pa.is_empty(), pb.is_empty()) {
        (true, true) => false,
        (true, false) => true,
        (false, true) => false,
        (false, false) => {
            for (x, y) in pa.iter().zip(pb.iter()) {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(nx), Ok(ny)) => nx.cmp(&ny),
                    (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                    (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                    _ => x.cmp(y),
                };
                if ord != std::cmp::Ordering::Equal {
                    return ord == std::cmp::Ordering::Greater;
                }
            }
            pa.len() > pb.len()
        }
    }
}

/// Ask npm (through the login shell, so registry config and auth apply) what the channel's
/// dist-tag points at, and remember it when it is newer than the running sidecar. Silent on
/// any failure — offline is "nothing to offer".
fn check_cezar_update(app: &AppHandle, shell: &Shell) {
    let running = shell.running_version.lock().unwrap().clone();
    let Some(running) = running else { return };
    let tag = release_tag();
    let mut command = login_shell(&format!("npm view {PACKAGE}@{tag} version --json 2>/dev/null"));
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let Ok(output) = command.output() else { return };
    let raw = String::from_utf8_lossy(&output.stdout);
    let latest = serde_json::from_str::<serde_json::Value>(raw.trim())
        .ok()
        .and_then(|value| match value {
            serde_json::Value::String(v) => Some(v),
            serde_json::Value::Array(items) => items.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).last(),
            _ => None,
        });
    let Some(latest) = latest else { return };
    let newer = version_newer(&latest, &running).then_some(latest);
    *shell.update_available.lock().unwrap() = newer.clone();
    if let (Some(version), Some(window)) = (newer, app.get_webview_window("main")) {
        offer_update(&window, &version);
    }
}

/// Tell the page there is something newer. The desktop-aware cockpit ignores this (it paints
/// its own button from the sidecar's check); a legacy one grows the button in its strip.
fn offer_update(window: &WebviewWindow, version: &str) {
    let _ = window.eval(&format!(
        "window.__CEZ_DESKTOP__ && window.__CEZ_DESKTOP__.showUpdate && window.__CEZ_DESKTOP__.showUpdate({})",
        js_string(version)
    ));
}

pub fn run() {
    let shell = Arc::new(Shell {
        child: Mutex::new(None),
        port: AtomicU16::new(0),
        restart_requested: AtomicBool::new(false),
        updating: AtomicBool::new(false),
        open_item: Mutex::new(None),
        versions_menu: Mutex::new(None),
        geometry_dirty: AtomicBool::new(false),
        running_version: Mutex::new(None),
        update_available: Mutex::new(None),
    });
    let shell_for_setup = shell.clone();
    let shell_for_menu = shell.clone();
    let shell_for_run = shell.clone();
    let shell_for_events = shell.clone();
    // SIGTERM/SIGINT (a `kill`, a logout, a supervisor of our own) never reach Tauri's Exit
    // event, and a cezar older than the parent watch would then outlive us — take the sidecar
    // down here, then leave.
    let shell_for_signal = shell.clone();
    let _ = ctrlc::set_handler(move || {
        if let Some(mut child) = shell_for_signal.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        std::process::exit(0);
    });

    let shell_for_state = shell.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![update_cezar_command])
        .setup(move |app| {
            app.manage(shell_for_state.clone());
            let handle = app.handle().clone();
            build_main_window(&handle)?;
            build_menu(&handle, &shell_for_setup)?;
            let shell = shell_for_setup.clone();
            let supervisor_handle = handle.clone();
            std::thread::spawn(move || supervise(supervisor_handle, shell));
            // Geometry writer: coalesces the burst of move/resize events into one file write.
            let shell = shell_for_setup.clone();
            let writer_handle = handle.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_millis(400));
                if shell.geometry_dirty.swap(false, Ordering::SeqCst) {
                    if let Some(window) = writer_handle.get_webview_window("main") {
                        save_geometry(&window);
                    }
                }
            });
            // The shell updates ITSELF rarely (spec 2026-09-25-desktop-distribution): check the
            // release manifest once per launch, in the background, and install silently — the
            // new shell takes over on the next launch. Never blocks startup; offline is a no-op.
            tauri::async_runtime::spawn(async move { check_shell_update(handle).await });
            Ok(())
        })
        .on_menu_event(move |app, event| match event.id().as_ref() {
            id if id.starts_with("use-version:") => {
                let target = id["use-version:".len()..].to_string();
                let app = app.clone();
                let shell = shell_for_menu.clone();
                std::thread::spawn(move || {
                    if let Some(window) = app.get_webview_window("main") {
                        splash_reset(&window, "Switching cezar…", &format!("Activating {target}."));
                    }
                    if !switch_version(&shell, &target) {
                        refresh_versions_menu(&app, &shell);
                    }
                });
            }
            "update-cezar" => {
                let app = app.clone();
                let shell = shell_for_menu.clone();
                std::thread::spawn(move || update_cezar(&app, &shell, "Updating cezar…"));
            }
            "open-browser" => {
                // The page the window is showing right now — route and all — so the browser tab
                // lands on the same task, not the cockpit's front door. The splash (a
                // `tauri://` page) falls back to the cockpit root on the live port.
                let current = app
                    .get_webview_window("main")
                    .and_then(|window| window.url().ok())
                    .filter(|url| url.scheme() == "http")
                    .map(|url| url.to_string().replacen("127.0.0.1", "localhost", 1));
                let port = shell_for_menu.port.load(Ordering::SeqCst);
                match current {
                    Some(url) => open_url(&url),
                    None if port != 0 => open_url(&format!("http://localhost:{port}")),
                    None => {}
                }
            }
            _ => {}
        })
        .on_window_event(move |window, event| {
            match event {
                WindowEvent::Moved(position) => {
                    if cfg!(debug_assertions) {
                        eprintln!("[geometry] moved to physical ({}, {}) scale {:?}", position.x, position.y, window.scale_factor().ok());
                    }
                    shell_for_events.geometry_dirty.store(true, Ordering::SeqCst);
                }
                WindowEvent::Resized(size) => {
                    if cfg!(debug_assertions) {
                        eprintln!("[geometry] resized to physical {}x{}", size.width, size.height);
                    }
                    shell_for_events.geometry_dirty.store(true, Ordering::SeqCst);
                }
                WindowEvent::CloseRequested { .. } => {
                    if let Some(webview) = window.get_webview_window("main") {
                        save_geometry(&webview);
                    }
                }
                _ => {}
            }
            // macOS convention: closing the window keeps the app (and the agents it is running)
            // alive in the Dock; Cmd+Q quits.
            #[cfg(target_os = "macos")]
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build the cezar desktop shell")
        .run(move |app, event| match event {
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            RunEvent::Exit => {
                if let Some(window) = app.get_webview_window("main") {
                    save_geometry(&window);
                }
                if let Some(mut child) = shell_for_run.child.lock().unwrap().take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            _ => {}
        });
}

/// Silent self-update of the shell. `CEZ_DESKTOP_NO_UPDATE=1` disables it (development builds
/// point at a checkout and must not replace themselves). Installed updates apply on the next
/// launch rather than restarting under the user, so a running task is never interrupted by
/// the shell — only the cockpit's own updater does that, and it asks first.
async fn check_shell_update(app: AppHandle) {
    use tauri_plugin_updater::UpdaterExt;
    if std::env::var_os("CEZ_DESKTOP_NO_UPDATE").is_some() || cfg!(debug_assertions) {
        return;
    }
    let Ok(updater) = app.updater() else { return };
    let Ok(Some(update)) = updater.check().await else { return };
    let _ = update.download_and_install(|_, _| {}, || {}).await;
}

/// Spawn → wait for health → show cockpit → wait for exit → relaunch on 75 (or on a requested
/// restart), report otherwise. A missing install is installed first.
fn supervise(app: AppHandle, shell: Arc<Shell>) {
    let window = loop {
        if let Some(window) = app.get_webview_window("main") {
            break window;
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    loop {
        let entry = match resolve_entry() {
            Some(entry) => entry,
            None => {
                // First launch on this machine: the supervisor puts a cezar in place itself.
                if !update_cezar(&app, &shell, "Installing cezar…") {
                    return;
                }
                match resolve_entry() {
                    Some(entry) => entry,
                    None => return,
                }
            }
        };
        let port = pick_port();
        shell.port.store(port, Ordering::SeqCst);
        if let Some(item) = shell.open_item.lock().unwrap().as_ref() {
            let _ = item.set_enabled(true);
        }
        let cwd = pick_cwd();
        refresh_versions_menu(&app, &shell);
        splash_reset(&window, "Starting cezar…", &entry.to_string_lossy());

        let log: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        let mut child = match spawn_sidecar(&entry, port, &cwd, log.clone()) {
            Ok(child) => child,
            Err(error) => {
                fail(&window, "cezar could not start", &error, "");
                return;
            }
        };
        let pid = child.id();

        let url = format!("http://127.0.0.1:{port}");
        if let Some(version) = wait_for_health(port, &mut child, HEALTH_TIMEOUT) {
            *shell.running_version.lock().unwrap() = Some(version);
            *shell.update_available.lock().unwrap() = None;
            if let Ok(parsed) = url::Url::parse(&url) {
                let _ = window.navigate(parsed);
            }
            probe_ipc(&window);
            // Registry check for the legacy strip's button: now, then every half hour.
            let app = app.clone();
            let shell = shell.clone();
            let generation = pid;
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(4));
                check_cezar_update(&app, &shell);
                std::thread::sleep(Duration::from_secs(30 * 60));
                // A new sidecar starts its own checker; this one retires.
                let current = shell.child.lock().unwrap().as_ref().map(|child| child.id());
                if current != Some(generation) {
                    return;
                }
            });
        } else {
            let tail = log.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
            fail(&window, "cezar did not come up", &format!("No answer on {url} within {}s (pid {pid}).", HEALTH_TIMEOUT.as_secs()), &tail);
            let _ = child.kill();
            return;
        }

        *shell.child.lock().unwrap() = Some(child);
        let status = loop {
            let mut guard = shell.child.lock().unwrap();
            match guard.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) => {}
                    Err(_) => break None,
                },
                None => break None, // taken by Exit — the app is quitting
            }
            drop(guard);
            std::thread::sleep(Duration::from_millis(250));
        };
        *shell.child.lock().unwrap() = None;

        let requested = shell.restart_requested.swap(false, Ordering::SeqCst);
        match status.and_then(|s| s.code()) {
            Some(RESTART_EXIT_CODE) => {
                splash_reset(&window, "Restarting cezar…", "Switching to the newly activated version.");
                continue;
            }
            _ if requested => {
                splash_reset(&window, "Restarting cezar…", "Switching to the newly installed version.");
                continue;
            }
            Some(code) => {
                let tail = log.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
                fail(&window, "cezar stopped", &format!("The cockpit process exited with status {code}."), &tail);
                return;
            }
            None => return,
        }
    }
}

// ---- installing / updating cezar from the shell ------------------------------------------

/// Install the channel's newest cezar into the managed layout and make it current — the same
/// layout and manifest the cockpit's own updater writes, so the two never disagree. Streams
/// npm's output to the splash. On success with a sidecar running, asks the supervisor loop to
/// relaunch. Returns whether the install succeeded.
fn update_cezar(app: &AppHandle, shell: &Shell, title: &str) -> bool {
    if shell.updating.swap(true, Ordering::SeqCst) {
        return false;
    }
    let Some(window) = app.get_webview_window("main") else {
        shell.updating.store(false, Ordering::SeqCst);
        return false;
    };
    let tag = release_tag();
    let versions = cezar_home().join("versions");
    splash_reset(&window, title, &format!("{PACKAGE}@{tag} → {}", versions.display()));

    // POSIX sh: the same steps as packages/cezar/src/self-update/installer.ts, staging dir and
    // all, written so they run against any node/npm on the user's login PATH.
    let script = format!(
        r#"set -e
V={versions}
mkdir -p "$V"
S="$V/.staging-shell-$$"
rm -rf "$S"; mkdir -p "$S"
npm install --prefix "$S" --omit=dev --no-audit --no-fund --no-package-lock --loglevel=notice {package}@{tag}
VER=$(node -p "require('$S/node_modules/{package}/package.json').version")
test -f "$S/node_modules/{package}/dist/index.js"
rm -rf "$V/$VER"
mv "$S" "$V/$VER"
printf '{{"version":"%s","source":"registry","installedAt":"%s"}}\n' "$VER" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$V/$VER/.cezar-install.json"
ln -sfn "$VER" "$V/current"
echo "installed $VER"
"#,
        versions = shell_quote(&versions.to_string_lossy()),
        package = PACKAGE,
        tag = tag,
    );
    let mut command = login_shell(&script);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let ok = match command.spawn() {
        Ok(mut child) => {
            let mut lines: Vec<String> = Vec::new();
            for reader in [
                child.stdout.take().map(|out| Box::new(out) as Box<dyn Read + Send>),
                child.stderr.take().map(|err| Box::new(err) as Box<dyn Read + Send>),
            ]
            .into_iter()
            .flatten()
            {
                // Sequential drain is fine: npm's chatter is small and stdout closes at the end.
                for line in BufReader::new(reader).lines().map_while(Result::ok) {
                    splash_log(&window, &line);
                    lines.push(line);
                }
            }
            match child.wait() {
                Ok(status) if status.success() => true,
                Ok(status) => {
                    fail(&window, "cezar could not be installed", &format!("npm exited with {status}. Is Node 20+ on your PATH?"), &lines.join("\n"));
                    false
                }
                Err(error) => {
                    fail(&window, "cezar could not be installed", &error.to_string(), "");
                    false
                }
            }
        }
        Err(error) => {
            fail(&window, "cezar could not be installed", &format!("could not start the login shell: {error}"), "");
            false
        }
    };
    shell.updating.store(false, Ordering::SeqCst);
    if ok {
        // A running sidecar is the OLD version: ask the loop to relaunch, then stop it.
        let mut guard = shell.child.lock().unwrap();
        if let Some(child) = guard.as_mut() {
            shell.restart_requested.store(true, Ordering::SeqCst);
            let _ = child.kill();
        }
    }
    ok
}

/// `updateChannel` from `~/.cezar/config.json` → the npm dist-tag; `latest` when unset.
fn release_tag() -> &'static str {
    let config = cezar_home().join("config.json");
    let channel = std::fs::read_to_string(config)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|json| json.get("updateChannel").and_then(|value| value.as_str()).map(str::to_owned));
    match channel.as_deref() {
        Some("nightly") => "nightly",
        _ => "latest",
    }
}

// ---- process -------------------------------------------------------------------------------

fn spawn_sidecar(entry: &Path, port: u16, cwd: &Path, log: Arc<Mutex<VecDeque<String>>>) -> Result<Child, String> {
    let script = format!("exec node {} serve --no-open --port {}", shell_quote(&entry.to_string_lossy()), port);
    let mut command = login_shell(&script);
    command
        .current_dir(cwd)
        .env("CEZ_DESKTOP", "1")
        .env("CEZ_SUPERVISED", "1")
        // cezar polls this pid and exits when it is gone — a force-quit of the shell never
        // leaves a headless cockpit behind (the Exit handler only runs on a clean quit).
        .env("CEZ_SUPERVISOR_PID", std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| format!("could not spawn the login shell: {error}"))?;
    for reader in [
        child.stdout.take().map(|out| Box::new(out) as Box<dyn Read + Send>),
        child.stderr.take().map(|err| Box::new(err) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let log = log.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(reader).lines().map_while(Result::ok) {
                let mut log = log.lock().unwrap();
                if log.len() >= LOG_TAIL {
                    log.pop_front();
                }
                log.push_back(line);
            }
        });
    }
    Ok(child)
}

/// A command through the user's LOGIN shell, so nvm/volta/homebrew PATH entries resolve exactly
/// as in their terminal. `cmd /C` on Windows (PoC — untested).
fn login_shell(script: &str) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.args(["/C", script]);
        command
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| if cfg!(target_os = "macos") { "/bin/zsh".into() } else { "/bin/bash".into() });
        let mut command = Command::new(shell);
        command.args(["-lc", script]);
        command
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();
    #[cfg(windows)]
    let _ = Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(url).spawn();
}

/// A one-shot HTTP GET to `/api/v1/health` without an HTTP client dependency. Answers the
/// reported `version` on a 200, None otherwise.
fn health_version(port: u16) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().ok()?, Duration::from_millis(500)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    stream.write_all(b"GET /api/v1/health HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").ok()?;
    let mut buffer = Vec::new();
    let _ = stream.read_to_end(&mut buffer);
    let text = String::from_utf8_lossy(&buffer);
    if !(text.starts_with("HTTP/1.0 200") || text.starts_with("HTTP/1.1 200")) {
        return None;
    }
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    let version = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|json| json.get("version").and_then(|v| v.as_str()).map(str::to_owned));
    Some(version.unwrap_or_default())
}

fn wait_for_health(port: u16, child: &mut Child, timeout: Duration) -> Option<String> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(version) = health_version(port) {
            return Some(version);
        }
        if let Ok(Some(_)) = child.try_wait() {
            return None; // died while starting
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    None
}

/// 4321 first (the port every README names), the next few when it is busy, then anything free.
fn pick_port() -> u16 {
    for port in PREFERRED_PORTS {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return port;
        }
    }
    TcpListener::bind("127.0.0.1:0").and_then(|listener| listener.local_addr()).map(|addr| addr.port()).unwrap_or(4321)
}

// ---- window geometry -----------------------------------------------------------------------
//
// Position and size are remembered in LOGICAL points (`~/.cezar/desktop-window.json`), never
// physical pixels: macOS's global coordinate space is in points, so a logical rectangle means
// the same thing on a Retina laptop panel and a 1x external display. (tauri-plugin-window-state
// saves physical pixels and restores the position before the size, so a window carried to a
// monitor with a different scale came back at the wrong size — the bug this replaces.)

fn geometry_path() -> PathBuf {
    cezar_home().join("desktop-window.json")
}

fn save_geometry(window: &WebviewWindow) {
    if window.is_minimized().unwrap_or(false) || !window.is_visible().unwrap_or(true) {
        return;
    }
    let Ok(scale) = window.scale_factor() else { return };
    let Ok(position) = window.outer_position() else { return };
    let Ok(size) = window.inner_size() else { return };
    let maximized = window.is_maximized().unwrap_or(false);
    let mut json = std::fs::read_to_string(geometry_path())
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !maximized {
        let position = position.to_logical::<f64>(scale);
        let size = size.to_logical::<f64>(scale);
        json["x"] = serde_json::json!(position.x);
        json["y"] = serde_json::json!(position.y);
        json["width"] = serde_json::json!(size.width);
        json["height"] = serde_json::json!(size.height);
    }
    json["maximized"] = serde_json::json!(maximized);
    let path = geometry_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, format!("{}\n", serde_json::to_string_pretty(&json).unwrap_or_default())).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Apply the saved rectangle when it still lands on a connected display; otherwise keep the
/// builder's centred default. Size first, then position, both logical.
fn apply_saved_geometry(window: &WebviewWindow) -> Option<(f64, f64)> {
    let raw = std::fs::read_to_string(geometry_path()).ok()?;
    let json = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let read = |key: &str| json.get(key).and_then(|value| value.as_f64());
    let (x, y, width, height) = (read("x")?, read("y")?, read("width")?, read("height")?);
    if !(width >= 720.0 && height >= 480.0) {
        return None;
    }
    let mut on_screen = false;
    if let Ok(monitors) = window.available_monitors() {
        for monitor in &monitors {
            let scale = monitor.scale_factor();
            let origin = monitor.position().to_logical::<f64>(scale);
            let extent = monitor.size().to_logical::<f64>(scale);
            // At least a title bar's worth of the window must be visible on this display.
            let hit = x + width > origin.x + 40.0
                && x < origin.x + extent.width - 40.0
                && y + 28.0 > origin.y
                && y < origin.y + extent.height - 40.0;
            if cfg!(debug_assertions) {
                eprintln!("[geometry] monitor at ({}, {}) {}x{} scale {scale} — saved ({x}, {y}) {width}x{height} hits: {hit}", origin.x, origin.y, extent.width, extent.height);
            }
            on_screen |= hit;
        }
    }
    let _ = window.set_size(tauri::LogicalSize::new(width, height));
    if on_screen {
        let _ = window.set_position(tauri::LogicalPosition::new(x, y));
    } else {
        let _ = window.center();
    }
    if json.get("maximized").and_then(|value| value.as_bool()).unwrap_or(false) {
        let _ = window.maximize();
    }
    on_screen.then_some((x, y))
}

// ---- locations -----------------------------------------------------------------------------

fn home_dir() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

fn cezar_home() -> PathBuf {
    std::env::var_os("CEZ_HOME").filter(|value| !value.is_empty()).map(PathBuf::from).unwrap_or_else(|| home_dir().join(".cezar"))
}

/// `CEZ_DESKTOP_ENTRY` (a checkout's `packages/cezar/dist/index.js` while developing), else the
/// managed layout's `current` entry.
fn resolve_entry() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("CEZ_DESKTOP_ENTRY").map(PathBuf::from) {
        if explicit.is_file() {
            return Some(explicit);
        }
    }
    let managed = cezar_home().join("versions").join("current").join("node_modules").join("@open-mercato").join("cezar").join("dist").join("index.js");
    managed.is_file().then_some(managed)
}

/// The boot folder: `CEZ_DESKTOP_CWD`, else the most recently opened registered project, else
/// the home directory (which cezar never registers as a project — the cockpit then shows the
/// registry).
fn pick_cwd() -> PathBuf {
    if let Some(explicit) = std::env::var_os("CEZ_DESKTOP_CWD").map(PathBuf::from) {
        if explicit.is_dir() {
            return explicit;
        }
    }
    let config = cezar_home().join("config.json");
    let fallback = home_dir();
    let Ok(raw) = std::fs::read_to_string(config) else { return fallback };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else { return fallback };
    let mut best: Option<(String, PathBuf)> = None;
    if let Some(projects) = json.get("projects").and_then(|value| value.as_array()) {
        for project in projects {
            let Some(root) = project.get("root").and_then(|value| value.as_str()) else { continue };
            let opened = project.get("lastOpenedAt").and_then(|value| value.as_str()).unwrap_or("").to_string();
            let path = PathBuf::from(root);
            if !path.is_dir() {
                continue;
            }
            if best.as_ref().map(|(when, _)| opened > *when).unwrap_or(true) {
                best = Some((opened, path));
            }
        }
    }
    best.map(|(_, path)| path).unwrap_or(fallback)
}

/// Debug builds with `CEZ_DESKTOP_DEBUG_IPC=1`: after the cockpit loads, exercise the two
/// things window dragging needs from the cockpit's (remote) origin — a direct IPC call, and
/// Tauri's injected drag handler reacting to a double-click on the title band — and print
/// whether each actually maximized the window. A drag that silently does nothing is otherwise
/// undiagnosable, and the page cannot report back any other way (document.title is not the
/// native title).
fn probe_ipc(window: &WebviewWindow) {
    if !cfg!(debug_assertions) || std::env::var_os("CEZ_DESKTOP_DEBUG_IPC").is_none() {
        return;
    }
    let window = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        let _ = window.unmaximize();
        std::thread::sleep(Duration::from_millis(500));
        let _ = window.eval("window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke('plugin:window|internal_toggle_maximize')");
        std::thread::sleep(Duration::from_millis(1500));
        let direct = window.is_maximized().unwrap_or(false);
        let _ = window.unmaximize();
        std::thread::sleep(Duration::from_millis(500));
        let _ = window.eval(
            r#"(function(){
              var el = document.querySelector('[data-slot=desktop-titlebar]') || document.body;
              var o = { bubbles: true, cancelable: true, button: 0, detail: 2, clientX: 400, clientY: 10 };
              el.dispatchEvent(new MouseEvent('mousedown', o));
              el.dispatchEvent(new MouseEvent('mouseup', o));
            })()"#,
        );
        std::thread::sleep(Duration::from_millis(1500));
        let via_drag_script = window.is_maximized().unwrap_or(false);
        let _ = window.unmaximize();
        std::thread::sleep(Duration::from_millis(500));
        // Legacy strip present? Signal it the same way (a maximize) — the only channel we have.
        let _ = window.eval("document.querySelector('[data-cez-legacy-titlebar]') && window.__TAURI_INTERNALS__.invoke('plugin:window|internal_toggle_maximize')");
        std::thread::sleep(Duration::from_millis(1500));
        let legacy_strip = window.is_maximized().unwrap_or(false);
        let _ = window.unmaximize();
        let own_strip_probe = window.eval("document.querySelector('[data-slot=desktop-titlebar]') && window.__TAURI_INTERNALS__.invoke('plugin:window|internal_toggle_maximize')");
        std::thread::sleep(Duration::from_millis(1500));
        let own_strip = own_strip_probe.is_ok() && window.is_maximized().unwrap_or(false);
        let _ = window.unmaximize();
        std::thread::sleep(Duration::from_secs(6));
        let _ = window.eval("document.querySelector('[data-cez-update-pill]') && window.__TAURI_INTERNALS__.invoke('plugin:window|internal_toggle_maximize')");
        std::thread::sleep(Duration::from_millis(1500));
        let legacy_update = window.is_maximized().unwrap_or(false);
        let _ = window.unmaximize();
        eprintln!("[probe] direct invoke maximized: {direct}; drag-region double-click maximized: {via_drag_script}; legacy strip injected: {legacy_strip}; cockpit's own strip: {own_strip}; legacy update button: {legacy_update}");
    });
}

// ---- splash page ---------------------------------------------------------------------------

fn splash_log(window: &WebviewWindow, line: &str) {
    let _ = window.eval(&format!("window.cezarSplash && window.cezarSplash.log({})", js_string(line)));
}

/// Back to the splash from the cockpit (a different origin), then set the message.
fn splash_reset(window: &WebviewWindow, title: &str, detail: &str) {
    if let Ok(url) = url::Url::parse(&format!("{}?title={}&detail={}", app_origin(), urlencode(title), urlencode(detail))) {
        let _ = window.navigate(url);
        // Navigation is asynchronous; give the page a beat before the first `eval` lands.
        std::thread::sleep(Duration::from_millis(400));
    }
}

fn fail(window: &WebviewWindow, title: &str, detail: &str, log: &str) {
    if let Ok(url) = url::Url::parse(&format!(
        "{}?error={}&title={}&log={}",
        app_origin(),
        urlencode(detail),
        urlencode(title),
        urlencode(log)
    )) {
        let _ = window.navigate(url);
    }
}

fn app_origin() -> &'static str {
    if cfg!(windows) {
        "http://tauri.localhost/index.html"
    } else {
        "tauri://localhost/index.html"
    }
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

fn urlencode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
