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

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};
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
}

/// Runs at document start on EVERY page the window loads — the splash and, after navigation,
/// the cockpit. The cockpit reads `data-cez-desktop` to make room for the traffic lights in
/// its sidebar header (see `packages/web/src/components/app-shell.tsx`); a browser tab on the
/// same server never sees it.
const INIT_SCRIPT: &str = r#"
  (function () {
    var platform = "__PLATFORM__";
    window.__CEZ_DESKTOP__ = { platform: platform };
    document.documentElement.dataset.cezDesktop = platform;
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
        .initialization_script(INIT_SCRIPT.replace("__PLATFORM__", platform_name()));
    // macOS: no title text; the traffic lights float over the 38px band the cockpit paints at
    // the top (`data-slot="desktop-titlebar"`), centred in it.
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(14.0, 13.0));
    builder.build()
}

fn build_menu(app: &AppHandle, shell: &Shell) -> tauri::Result<()> {
    let update_item = MenuItem::with_id(app, "update-cezar", "Update cezar to latest…", true, None::<&str>)?;
    let open_item = MenuItem::with_id(app, "open-browser", "Open cockpit in browser", true, None::<&str>)?;
    *shell.open_item.lock().unwrap() = Some(open_item.clone());
    let app_menu = Submenu::with_items(
        app,
        "cezar",
        true,
        &[
            &PredefinedMenuItem::about(app, None, None)?,
            &PredefinedMenuItem::separator(app)?,
            &update_item,
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

pub fn run() {
    let shell = Arc::new(Shell {
        child: Mutex::new(None),
        port: AtomicU16::new(0),
        restart_requested: AtomicBool::new(false),
        updating: AtomicBool::new(false),
        open_item: Mutex::new(None),
    });
    let shell_for_setup = shell.clone();
    let shell_for_menu = shell.clone();
    let shell_for_run = shell.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        // Remembers the window's position and size across launches (and across monitors), so
        // the app opens where it was left instead of centred on the main display.
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .setup(move |app| {
            let handle = app.handle().clone();
            build_main_window(&handle)?;
            build_menu(&handle, &shell_for_setup)?;
            let shell = shell_for_setup.clone();
            let supervisor_handle = handle.clone();
            std::thread::spawn(move || supervise(supervisor_handle, shell));
            // The shell updates ITSELF rarely (spec 2026-09-25-desktop-distribution): check the
            // release manifest once per launch, in the background, and install silently — the
            // new shell takes over on the next launch. Never blocks startup; offline is a no-op.
            tauri::async_runtime::spawn(async move { check_shell_update(handle).await });
            Ok(())
        })
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "update-cezar" => {
                let app = app.clone();
                let shell = shell_for_menu.clone();
                std::thread::spawn(move || update_cezar(&app, &shell, "Updating cezar…"));
            }
            "open-browser" => {
                let port = shell_for_menu.port.load(Ordering::SeqCst);
                if port != 0 {
                    open_url(&format!("http://localhost:{port}"));
                }
            }
            _ => {}
        })
        .on_window_event(|window, event| {
            // macOS convention: closing the window keeps the app (and the agents it is running)
            // alive in the Dock; Cmd+Q quits.
            #[cfg(target_os = "macos")]
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (window, event);
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
                let _ = app;
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
            let _ = item.set_text(format!("Open http://localhost:{port} in browser"));
        }
        let cwd = pick_cwd();
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
        if wait_for_health(port, &mut child, HEALTH_TIMEOUT) {
            if let Ok(parsed) = url::Url::parse(&url) {
                let _ = window.navigate(parsed);
            }
            probe_ipc(&window);
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

/// A one-shot HTTP GET to `/api/v1/health` without an HTTP client dependency.
fn health_ok(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().unwrap(), Duration::from_millis(500)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    if stream.write_all(b"GET /api/v1/health HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").is_err() {
        return false;
    }
    let mut buffer = Vec::new();
    let _ = stream.read_to_end(&mut buffer);
    let head = String::from_utf8_lossy(&buffer);
    head.starts_with("HTTP/1.0 200") || head.starts_with("HTTP/1.1 200")
}

fn wait_for_health(port: u16, child: &mut Child, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if health_ok(port) {
            return true;
        }
        if let Ok(Some(_)) = child.try_wait() {
            return false; // died while starting
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
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
        eprintln!("[probe] direct invoke maximized: {direct}; drag-region double-click maximized: {via_drag_script}");
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
