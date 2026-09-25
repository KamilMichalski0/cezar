//! cezar desktop (PoC): a native window around the managed cezar install.
//!
//! The shell is deliberately thin. It resolves the entry file of the ACTIVE managed version
//! (`~/.cezar/versions/current`), runs it as a sidecar through the user's login shell (so the
//! `node`, `claude`, `gh` on their PATH are the ones cezar sees — a GUI app inherits none of
//! that), waits for `/api/v1/health`, and points the webview at the cockpit. Everything the
//! cockpit does — including updating cezar — happens in the sidecar: an update ends with the
//! sidecar exiting `75` (`CEZ_SUPERVISED=1`), and the supervisor loop below relaunches it,
//! which picks up the freshly activated version. So the desktop app never needs a release to
//! ship a cezar update; only the shell itself would.
//!
//! No Tauri IPC is exposed to the cockpit (it is a remote origin); the splash page is driven
//! with `eval`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager, RunEvent, WebviewWindow, WindowEvent};

/// Exit status the sidecar uses to say "relaunch me" after a self-update (EX_TEMPFAIL).
const RESTART_EXIT_CODE: i32 = 75;
const HEALTH_TIMEOUT: Duration = Duration::from_secs(60);
const LOG_TAIL: usize = 60;

type SharedChild = Arc<Mutex<Option<Child>>>;

pub fn run() {
    let child: SharedChild = Arc::new(Mutex::new(None));
    let child_for_setup = child.clone();
    let child_for_run = child.clone();

    tauri::Builder::default()
        .setup(move |app| {
            let handle = app.handle().clone();
            let child = child_for_setup.clone();
            std::thread::spawn(move || supervise(handle, child));
            Ok(())
        })
        .on_window_event(|window, event| {
            // macOS convention: closing the window keeps the app (and the agents it is running)
            // alive in the Dock; Cmd+Q quits. The default menu Tauri installs carries Quit.
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
                if let Some(mut child) = child_for_run.lock().unwrap().take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
            _ => {}
        });
}

/// Spawn → wait for health → show cockpit → wait for exit → relaunch on 75, report otherwise.
fn supervise(app: AppHandle, shared: SharedChild) {
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
                fail(
                    &window,
                    "cezar is not installed",
                    "Install it once from a terminal, then reopen this app:",
                    "npx cezar-cli install",
                );
                return;
            }
        };
        let port = free_port();
        let cwd = pick_cwd();
        splash(&window, "Starting cezar…", &format!("{}", entry.display()));

        let log: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        let mut child = match spawn_sidecar(&entry, port, &cwd, log.clone()) {
            Ok(child) => child,
            Err(error) => {
                fail(&window, "cezar could not start", &error, "");
                return;
            }
        };
        let pid = child.id();
        *shared.lock().unwrap() = None;

        let url = format!("http://127.0.0.1:{port}");
        let healthy = wait_for_health(port, &mut child, HEALTH_TIMEOUT);
        if healthy {
            if let Ok(parsed) = url::Url::parse(&url) {
                let _ = window.navigate(parsed);
            }
        } else {
            let tail = log.lock().unwrap().iter().cloned().collect::<Vec<_>>().join("\n");
            fail(&window, "cezar did not come up", &format!("No answer on {url} within {}s (pid {pid}).", HEALTH_TIMEOUT.as_secs()), &tail);
            let _ = child.kill();
            return;
        }

        *shared.lock().unwrap() = Some(child);
        let status = loop {
            let mut guard = shared.lock().unwrap();
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
        *shared.lock().unwrap() = None;

        match status.and_then(|s| s.code()) {
            Some(RESTART_EXIT_CODE) => {
                splash_reset(&window, "Restarting cezar…", "Switching to the newly activated version.");
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

// ---- process -------------------------------------------------------------------------------

fn spawn_sidecar(entry: &Path, port: u16, cwd: &Path, log: Arc<Mutex<VecDeque<String>>>) -> Result<Child, String> {
    let mut command = login_shell_command(entry, port);
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

/// `node <entry> serve --no-open --port <port>` through the user's login shell, so nvm/volta/
/// homebrew PATH entries resolve exactly as in their terminal.
fn login_shell_command(entry: &Path, port: u16) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd");
        command.args(["/C", "node", &entry.to_string_lossy(), "serve", "--no-open", "--port", &port.to_string()]);
        command
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| if cfg!(target_os = "macos") { "/bin/zsh".into() } else { "/bin/bash".into() });
        let script = format!("exec node {} serve --no-open --port {}", shell_quote(&entry.to_string_lossy()), port);
        let mut command = Command::new(shell);
        command.args(["-lc", &script]);
        command
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
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
    String::from_utf8_lossy(&buffer).starts_with("HTTP/1.0 200") || String::from_utf8_lossy(&buffer).starts_with("HTTP/1.1 200")
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

fn free_port() -> u16 {
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

/// The boot folder: the most recently opened registered project, else the home directory
/// (which cezar never registers as a project — the cockpit then shows the registry).
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

// ---- splash page ---------------------------------------------------------------------------

fn splash(window: &WebviewWindow, title: &str, detail: &str) {
    let _ = window.eval(&format!("window.cezarSplash && window.cezarSplash.set({}, {})", js_string(title), js_string(detail)));
}

/// Back to the splash from the cockpit (a different origin), then set the message.
fn splash_reset(window: &WebviewWindow, title: &str, detail: &str) {
    if let Ok(url) = url::Url::parse(&format!("{}?title={}&detail={}", app_origin(), urlencode(title), urlencode(detail))) {
        let _ = window.navigate(url);
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
