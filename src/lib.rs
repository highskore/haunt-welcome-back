//! Welcome back: step away for a while and a sheet greets you with who finished, who is stuck and
//! where to start; the ⌘K `back` mode keeps the longer history. Built only from agent hook events;
//! history lives in kv.
// wit-bindgen's flattened `notify` import has 14 params.
#![allow(clippy::too_many_arguments)]

wit_bindgen::generate!({ path: "wit/haunt-plugin.wit", world: "plugin" });

mod away;
mod back;
// Shared with the other fleet-following plugins; not every helper is used here.
#[allow(dead_code)]
mod fleet;

use std::sync::Mutex;

use back::{Entry, Kind, Settings, State, clean, project};
use haunt::plugin::log::log;
use haunt::plugin::types::LogLevel;
use haunt::plugin::panel::{self, ButtonNode, Node, PanelNode, Placement, TextNode, Tone};
use haunt::plugin::{kv, palette};

pub const MODE: &str = "back";
const KEY: &str = "state";

const SHEET: &str = "away";

static SETTINGS: Mutex<Option<Settings>> = Mutex::new(None);
static AWAY: Mutex<Option<away::Away>> = Mutex::new(None);

fn with_away<R>(f: impl FnOnce(&mut away::Away) -> R) -> R {
    f(AWAY.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(away::Away::default))
}

/// The sheet's title and nodes, or nothing when no agent did anything.
pub fn sheet(a: &away::Away, away_ms: u64, now: u64) -> Option<(String, Vec<PanelNode>)> {
    let lines = a.lines(now);
    if lines.is_empty() {
        return None;
    }
    let mut nodes: Vec<PanelNode> =
        away::render(&lines).into_iter().map(|text| PanelNode { parent: None, node: Node::Text(TextNode { text, tone: Tone::Text, mono: true, bold: false }) }).collect();
    let buttons = nodes.len() as u32;
    nodes.push(PanelNode { parent: None, node: Node::Hstack });
    if let Some((surface, name)) = a.start_with() {
        nodes.push(PanelNode { parent: Some(buttons), node: Node::Button(ButtonNode { label: format!("Start with {name}"), action: "start".into(), primary: true, surface: Some(surface.to_owned()) }) });
    }
    nodes.push(PanelNode { parent: Some(buttons), node: Node::Button(ButtonNode { label: "Dismiss".into(), action: "dismiss".into(), primary: false, surface: None }) });
    let m = away_ms / 60_000;
    let gone = if m < 60 { format!("{m} min") } else { format!("{} h {} min", m / 60, m % 60) };
    Some((format!("While you were away · {gone}"), nodes))
}

struct Plugin;

fn settings() -> Settings {
    SETTINGS.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn load() -> State {
    kv::get(KEY)
        .map(|j| {
            State::parse(&j).unwrap_or_else(|e| {
                log(LogLevel::Warn, &format!("welcome-back: saved history unreadable ({e}); starting fresh"));
                State::default()
            })
        })
        .unwrap_or_default()
}

fn save(st: &State) {
    if let Err(e) = kv::set(KEY, &st.to_json()) {
        log(LogLevel::Warn, &format!("welcome-back: history not saved: {e}"));
    }
}

/// The entry an event adds, if any. Unverified events are skipped unless the user opted in.
pub fn entry(event: &AgentEvent, s: &Settings, now: u64) -> Option<Entry> {
    if !event.verified && !s.unverified {
        return None;
    }
    let kind = Kind::of(&event.kind, event.summary.as_deref())?;
    let session = event.session_id.as_deref().map(clean).filter(|s| !s.is_empty())?;
    let project = project(event.cwd.as_deref()).map(|p| clean(&p)).unwrap_or_else(|| "agent".into());
    let text = (kind == Kind::TaskAdded).then(|| event.summary.as_deref().map(clean)).flatten().filter(|t| !t.is_empty());
    Some(Entry { at: now, session, project, kind, text })
}

impl Guest for Plugin {
    fn init(json: String) {
        *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(Settings::parse(&json));
        palette::mode(MODE, "What your agents did while you were away");
    }

    fn on_settings_changed(json: String) {
        *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(Settings::parse(&json));
    }

    fn on_event(event: AgentEvent) {
        let (s, now) = (settings(), now_ms());
        if let Some(surface) = event.surface.as_deref()
            && (event.verified || s.unverified)
        {
            let ev = fleet::Ev { surface, kind: &event.kind, cwd: event.cwd.as_deref(), summary: event.summary.as_deref(), subagent: event.agent_id.is_some() };
            let status = (event.kind == "task_updated").then_some(event.summary.as_deref()).flatten();
            with_away(|a| a.apply(&ev, event.command.as_deref(), status, now));
        }
        if let Some(e) = entry(&event, &s, now) {
            let mut st = load();
            st.record(e, now, s.keep_days);
            save(&st);
        }
    }

    fn palette_items(mode: String, query: String) -> Vec<PaletteItem> {
        if mode != MODE {
            return Vec::new();
        }
        let now = now_ms();
        let mut st = load();
        st.look(now);
        save(&st);
        st.view(&settings(), now, &query)
            .into_iter()
            .map(|i| PaletteItem { id: i.id, title: i.title, subtitle: i.subtitle, at_ms: i.at, mark: None, surface: None })
            .collect()
    }

    fn on_action(action_id: String, _payload: String) {
        // "start" brings its agent's tab up on its own, through the button's surface.
        if action_id == "dismiss" || action_id == "start" {
            panel::close(SHEET);
        }
    }

    fn row_menu(_surface: String) -> Vec<MenuItem> {
        Vec::new()
    }

    fn on_row_moved(_surface: String, _group: String, _before: Option<String>) {}

    fn on_theme_changed() {}

    fn on_tick(_now_ms: u64) {}

    fn on_exec(_job: u64, _outcome: Result<haunt::plugin::exec::Output, String>) {}

    fn on_away_changed(away: bool, away_ms: u64) {
        let (s, now) = (settings(), now_ms());
        with_away(|a| {
            if away {
                a.leave(now);
                return;
            }
            let long = away_ms >= s.min_away_min.clamp(1, 24 * 60) as u64 * 60_000;
            if s.sheet
                && long
                && let Some((title, nodes)) = sheet(a, away_ms, now)
                && let Err(e) = panel::show(SHEET, Placement::Sheet, &title, &nodes)
            {
                log(LogLevel::Warn, &format!("welcome-back: sheet not shown: {e}"));
            }
            a.since = None;
        });
    }

    fn on_hover_hold(_held: bool, _surface: String) {}

    fn on_fs_tree(_job: u64, _outcome: Result<haunt::plugin::fs_list::TreeResult, String>) {}
}

export!(Plugin);

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, summary: Option<&str>, verified: bool) -> AgentEvent {
        AgentEvent {
            kind: kind.into(),
            session_id: Some("sess-1".into()),
            agent_id: None,
            cwd: Some("/Users/me/code/vibes".into()),
            summary: summary.map(Into::into),
            verified,
            surface: Some("s1".into()),
            tool: None,
            command: None,
            file_path: None,
            task_id: None,
            request_key: None,
        }
    }

    #[test]
    fn verified_events_become_entries_with_their_project() {
        let e = entry(&event("task_created", Some("Write\nthe README"), true), &Settings::default(), 5).unwrap();
        assert_eq!((e.at, e.session.as_str(), e.project.as_str(), e.kind), (5, "sess-1", "vibes", Kind::TaskAdded));
        assert_eq!(e.text.as_deref(), Some("Write the README"));
        assert_eq!(entry(&event("stop", None, true), &Settings::default(), 5).unwrap().text, None);
    }

    #[test]
    fn unverified_events_are_skipped_unless_opted_in() {
        assert!(entry(&event("stop", None, false), &Settings::default(), 5).is_none());
        let opted = Settings { unverified: true, ..Settings::default() };
        assert!(entry(&event("stop", None, false), &opted, 5).is_some());
    }

    #[test]
    fn events_without_a_session_are_skipped() {
        let mut ev = event("stop", None, true);
        ev.session_id = None;
        assert!(entry(&ev, &Settings::default(), 5).is_none());
    }

    #[test]
    fn the_sheet_has_the_mockups_title_lines_and_buttons() {
        let mut a = away::Away::default();
        let ev = |s, kind, cwd| fleet::Ev { surface: s, kind, cwd: Some(cwd), ..fleet::Ev::default() };
        a.apply(&ev("d", "prompt_submit", "/w/docs"), None, None, 0);
        a.apply(&ev("a", "prompt_submit", "/w/api"), None, None, 0);
        a.leave(1000);
        assert!(sheet(&a, 23 * 60_000, 2000).is_none(), "nothing happened");
        a.apply(&ev("d", "stop", "/w/docs"), None, None, 240_000);
        a.apply(&ev("a", "permission_seen", "/w/api"), Some("git push"), None, 300_000);
        let (title, nodes) = sheet(&a, 23 * 60_000, 1_020_000).unwrap();
        assert_eq!(title, "While you were away · 23 min");
        let texts: Vec<_> = nodes.iter().filter_map(|n| if let Node::Text(t) = &n.node { Some(t.text.as_str()) } else { None }).collect();
        assert_eq!(texts, ["✓ docs  finished · Done in 4m", "! api   waiting 12m · git push?"]);
        let buttons: Vec<_> = nodes.iter().filter_map(|n| if let Node::Button(b) = &n.node { Some((b.label.as_str(), b.action.as_str(), b.primary, b.surface.as_deref(), n.parent)) } else { None }).collect();
        assert_eq!(buttons, [("Start with api", "start", true, Some("a"), Some(2)), ("Dismiss", "dismiss", false, None, Some(2))], "Start brings api's tab up");
    }

    #[test]
    fn manifest_asks_only_for_the_events_it_reads_and_defaults_match() {
        let m = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/haunt-plugin.toml")).unwrap();
        let t: toml::Table = toml::from_str(&m).unwrap();
        let caps = t["capabilities"].as_table().unwrap();
        assert_eq!(caps.keys().collect::<Vec<_>>(), ["away", "event_details", "events", "panel"]);
        let events: Vec<_> = caps["events"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(!events.contains(&"agent.*"), "{events:?}");
        let defaults: serde_json::Map<String, serde_json::Value> =
            t["settings"].as_table().unwrap().iter().map(|(k, v)| (k.clone(), serde_json::to_value(&v["default"]).unwrap())).collect();
        assert_eq!(Settings::parse(&serde_json::Value::Object(defaults).to_string()), Settings::default());
    }
}
