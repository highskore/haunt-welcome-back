//! End to end against a real haunt core. Copy into haunt's `cli/tests/` and run
//! `cargo test -p haunt-cli --features plugins --test welcome_back` (see README).
#![cfg(feature = "plugins")]

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use haunt_core::plugin_panel::{Panel, PanelNodeKind, PanelPlacement};
use haunt_core::plugin_settings::PluginConfig;
use haunt_core::plugin_store::{PluginSet, Store, attach};
use haunt_core::plugins::{HostRequest, PluginHost};
use haunt_core::{Core, SurfaceEnv};

fn repo() -> PathBuf {
    std::env::var_os("HAUNT_PLUGINS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../haunt-plugins"))
        .join("haunt-welcome-back")
}

fn until(what: &str, f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(5), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

const ID: &str = "dev.haunt.welcome-back";

fn send(sock: &mut UnixStream, env: &SurfaceEnv, nonce: bool, body: &str) {
    send_in(sock, env, nonce, "/tmp/vibes", body);
}

fn send_in(sock: &mut UnixStream, env: &SurfaceEnv, nonce: bool, cwd: &str, body: &str) {
    let nonce = if nonce { format!(r#","nonce":"{}""#, env.nonce) } else { String::new() };
    let line = format!(r#"{{"v":1,"agent":"claude","surface_id":"{}"{nonce},"session_id":"s1","cwd":"{cwd}",{body}}}"#, env.surface_id);
    sock.write_all(format!("{line}\n").as_bytes()).unwrap();
}

/// Builds and installs the plugin into a fresh home; `None` when the repo isn't there.
fn install(name: &str) -> Option<PathBuf> {
    let dir = repo();
    if !dir.join("haunt-plugin.toml").is_file() {
        eprintln!("skipped: haunt-welcome-back not found; set HAUNT_PLUGINS_DIR");
        return None;
    }
    let home = PathBuf::from(format!("/tmp/haunt-fp-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .current_dir(&dir)
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .unwrap();
    assert!(status.success());
    let o = Command::new(env!("CARGO_BIN_EXE_haunt")).args(["plugin", "install", dir.to_str().unwrap(), "--yes"]).env("HAUNT_HOME", &home).output().unwrap();
    assert!(o.status.success(), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    Some(home)
}

fn sheets(set: &Mutex<PluginSet>) -> Vec<Option<Panel>> {
    set.lock().unwrap().take_requests().into_iter().filter_map(|r| match r {
        HostRequest::ShowPanel { plugin_id, panel, .. } if plugin_id == ID => Some(Some(panel)),
        HostRequest::ClosePanel { plugin_id, id } if plugin_id == ID && id == "away" => Some(None),
        _ => None,
    }).collect()
}

#[test]
fn back_mode_reports_what_the_agents_did() {
    let Some(home) = install("back") else { return };
    let core = Core::new(&home);
    core.start().unwrap();
    let env = core.register_surface("/tmp").unwrap();
    let set = Store::open(&home).unwrap().load_all(&PluginHost::new().unwrap(), Some(&core.handle())).unwrap();
    assert!(set.errors().is_empty(), "{:?}", set.errors());
    let set = Arc::new(Mutex::new(set));
    attach(&core, &set);
    let modes: Vec<_> = set.lock().unwrap().palette_items(None, "").into_iter().filter(|i| i.is_mode).map(|i| i.id).collect();
    assert_eq!(modes, ["back"]);

    let mut sock = UnixStream::connect(&env.socket_path).unwrap();
    send(&mut sock, &env, true, r#""event":"session_start""#);
    send(&mut sock, &env, true, r#""event":"prompt_submit","query":"go""#);
    send(&mut sock, &env, true, r#""event":"task_created","task_id":"1","task_subject":"Write the parser""#);
    send(&mut sock, &env, true, r#""event":"task_updated","task_id":"1","task_status":"completed""#);
    send(&mut sock, &env, false, r#""event":"stop_failure""#);
    send(&mut sock, &env, true, r#""event":"stop""#);
    let back = |q: &str| set.lock().unwrap().palette_items(Some("back"), q);
    until("the stop to arrive", || back("").iter().any(|i| i.title == "vibes finished"));

    let items = back("");
    let titles: Vec<_> = items.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles[0], "So far: 1 turn finished, 1 task completed, 1 new session", "the forged failure isn't counted");
    assert_eq!(titles[1], "vibes · finished · 1 task completed · 1 added");
    assert_eq!(items[1].subtitle.as_deref(), Some("latest task: Write the parser"));
    assert_eq!(&titles[2..], ["vibes finished", "vibes completed a task", "vibes new task: Write the parser", "vibes started"]);
    assert!(items[1].at_ms.is_some_and(|t| t > 0));

    let config = PluginConfig::parse("[plugins.\"dev.haunt.welcome-back\"]\ntimeline = false\ntasks = false\n");
    assert!(set.lock().unwrap().set_config(&config));
    let titles: Vec<_> = back("").into_iter().map(|i| i.title).collect();
    assert_eq!(titles, ["So far: 1 turn finished, 1 new session", "vibes · finished"]);

    drop(core);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn coming_back_after_a_while_shows_who_finished_and_who_waits() {
    let Some(home) = install("away") else { return };
    let core = Core::new(&home);
    core.start().unwrap();
    let (docs, api) = (core.register_surface("/tmp").unwrap(), core.register_surface("/tmp").unwrap());
    let set = Store::open(&home).unwrap().load_all(&PluginHost::new().unwrap(), Some(&core.handle())).unwrap();
    assert!(set.errors().is_empty(), "{:?}", set.errors());
    let set = Arc::new(Mutex::new(set));
    attach(&core, &set);
    let mut sock = UnixStream::connect(&docs.socket_path).unwrap();
    for (env, cwd) in [(&docs, "/tmp/docs"), (&api, "/tmp/api")] {
        send_in(&mut sock, env, true, cwd, r#""event":"prompt_submit","query":"go""#);
    }

    let (t, min) = (Instant::now(), |m: u64| Duration::from_secs(m * 60));
    assert_eq!(set.lock().unwrap().presence(true, 0, t), Some((true, 0)), "a lock is away at once");
    send_in(&mut sock, &docs, true, "/tmp/docs", r#""event":"stop""#);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(set.lock().unwrap().presence(false, 0, t + min(5)), Some((false, 300_000)));
    assert_eq!(sheets(&set), [], "5 minutes is too short for a sheet");

    set.lock().unwrap().presence(true, 0, t + min(5));
    send_in(&mut sock, &docs, true, "/tmp/docs", r#""event":"prompt_submit","query":"again""#);
    send_in(&mut sock, &docs, true, "/tmp/docs", r#""event":"stop""#);
    send_in(&mut sock, &api, true, "/tmp/api", r#""event":"permission_seen","tool_name":"Bash","tool_input":{"command":"git push"}"#);
    send_in(&mut sock, &api, false, "/tmp/api", r#""event":"stop""#);
    std::thread::sleep(Duration::from_millis(300));
    set.lock().unwrap().presence(false, 0, t + min(30));
    assert_eq!(sheets(&set), [], "the sheet waits until you switch to Haunt");
    let active = Instant::now();
    set.lock().unwrap().app_activated(active);
    set.lock().unwrap().return_sheets_due(false, active + Duration::from_secs(5));

    let shown = sheets(&set);
    let [Some(p)] = shown.as_slice() else { panic!("{shown:?}") };
    assert_eq!((p.id.as_str(), p.placement, p.title.as_str()), ("away", PanelPlacement::Sheet, "While you were away · 25 min"));
    let text: Vec<_> = p.nodes.iter().filter_map(|n| if let PanelNodeKind::Text { text, .. } = &n.kind { Some(text.as_str()) } else { None }).collect();
    assert_eq!(text.len(), 2, "{text:?}");
    assert!(text[0].starts_with("✓ docs") && text[0].contains("finished"), "{text:?}");
    assert!(text[1].starts_with("! api") && text[1].contains("git push"), "the forged stop doesn't count: {text:?}");
    let buttons: Vec<_> = p.nodes.iter().filter_map(|n| if let PanelNodeKind::Button { label, action, surface, .. } = &n.kind { Some((label.as_str(), action.as_str(), surface.clone())) } else { None }).collect();
    assert_eq!(buttons, [("Start with api", "start", Some(api.surface_id.clone())), ("Dismiss", "dismiss", None)]);

    set.lock().unwrap().panel_action(ID, "away", p.generation, "start").unwrap();
    assert_eq!(sheets(&set), [None]);

    drop(core);
    let _ = std::fs::remove_dir_all(&home);
}
