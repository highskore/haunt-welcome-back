//! The history and the `back` view, free of host calls so it tests natively.

use serde::{Deserialize, Serialize};

/// Palette opens closer together than this are one look, so typing a query doesn't move "since".
pub const LOOK_GAP_MS: u64 = 90_000;
pub const MAX_ENTRIES: usize = 1000;
const TEXT_MAX: usize = 80;
const DAY_MS: u64 = 86_400_000;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub finished: bool,
    pub failed: bool,
    pub waiting: bool,
    pub tasks: bool,
    pub sessions: bool,
    pub per_session: bool,
    pub timeline: bool,
    pub unverified: bool,
    pub keep_days: i64,
    /// The "While you were away" sheet when you come back.
    pub sheet: bool,
    /// Minutes away before the sheet shows.
    pub min_away_min: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            finished: true,
            failed: true,
            waiting: true,
            tasks: true,
            sessions: true,
            per_session: true,
            timeline: true,
            unverified: false,
            keep_days: 7,
            sheet: true,
            min_away_min: 20,
        }
    }
}

impl Settings {
    pub fn parse(json: &str) -> Self {
        serde_json::from_str(json).unwrap_or_default()
    }

    fn shows(&self, kind: Kind) -> bool {
        match kind {
            Kind::Started => self.sessions,
            Kind::Turn => false,
            Kind::Finished => self.finished,
            Kind::Failed => self.failed,
            Kind::Approval | Kind::Question => self.waiting,
            Kind::TaskAdded | Kind::TaskDone => self.tasks,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Started,
    Turn,
    Finished,
    Failed,
    Approval,
    Question,
    TaskAdded,
    TaskDone,
}

impl Kind {
    /// The event's `kind` and `summary` as haunt sends them; `None` for events this plugin ignores.
    pub fn of(kind: &str, summary: Option<&str>) -> Option<Self> {
        Some(match kind {
            "session_start" => Self::Started,
            "prompt_submit" => Self::Turn,
            "stop" => Self::Finished,
            "stop_failure" => Self::Failed,
            "permission_seen" => Self::Approval,
            "question_asked" => Self::Question,
            "task_created" => Self::TaskAdded,
            "task_updated" if summary == Some("completed") => Self::TaskDone,
            _ => return None,
        })
    }

    fn phrase(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Turn => "working",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Approval => "asked for approval",
            Self::Question => "asked you a question",
            Self::TaskAdded => "new task",
            Self::TaskDone => "completed a task",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub at: u64,
    pub session: String,
    pub project: String,
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub entries: Vec<Entry>,
    /// The last time the `back` view was open.
    pub last_look: u64,
    /// Where the current look starts: the end of the look before it.
    pub since: u64,
}

/// One palette row, before it becomes the WIT type.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub at: Option<u64>,
}

/// The last path component, as haunt names rows; `None` for an empty or root path.
pub fn project(cwd: Option<&str>) -> Option<String> {
    let c = cwd?.trim_end_matches('/');
    c.rsplit('/').next().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Agent text is untrusted: keep it on one line, without hidden characters, and short.
pub fn clean(s: &str) -> String {
    let one: String = s.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).filter(|c| !c.is_control() && !is_format(*c)).collect();
    let one = one.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= TEXT_MAX { one } else { format!("{}…", one.chars().take(TEXT_MAX - 1).collect::<String>()) }
}

fn is_format(c: char) -> bool {
    matches!(c, '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}' | '\u{FEFF}' | '\u{E0000}'..='\u{E007F}')
}

impl State {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn record(&mut self, entry: Entry, now: u64, keep_days: i64) {
        self.entries.push(entry);
        let keep = DAY_MS.saturating_mul(keep_days.max(1) as u64);
        self.entries.retain(|e| now.saturating_sub(e.at) <= keep);
        let over = self.entries.len().saturating_sub(MAX_ENTRIES);
        self.entries.drain(..over);
    }

    /// Opening the view: a new look starts where the last one ended.
    pub fn look(&mut self, now: u64) {
        if now.saturating_sub(self.last_look) > LOOK_GAP_MS {
            self.since = self.last_look;
        }
        self.last_look = now;
    }

    pub fn view(&self, s: &Settings, now: u64, query: &str) -> Vec<Item> {
        let q = query.trim().to_lowercase();
        let fresh: Vec<&Entry> = self.entries.iter().filter(|e| e.at > self.since).collect();
        let matches = |e: &Entry| q.is_empty() || e.project.to_lowercase().contains(&q) || e.text.as_deref().is_some_and(|t| t.to_lowercase().contains(&q));
        let shown: Vec<&Entry> = fresh.iter().copied().filter(|e| matches(e)).collect();

        let mut out = vec![self.headline(s, &shown, now)];
        if s.per_session {
            out.extend(sessions(&shown).iter().map(|ss| ss.item(s, &self.labels(&shown))));
        }
        if s.timeline {
            let labels = self.labels(&shown);
            out.extend(shown.iter().rev().filter(|e| s.shows(e.kind)).enumerate().map(|(i, e)| Item {
                id: format!("e:{}:{i}", e.at),
                title: match &e.text {
                    Some(t) => format!("{} {}: {t}", label(&labels, e), e.kind.phrase()),
                    None => format!("{} {}", label(&labels, e), e.kind.phrase()),
                },
                subtitle: None,
                at: Some(e.at),
            }));
        }
        out
    }

    fn headline(&self, s: &Settings, shown: &[&Entry], now: u64) -> Item {
        let n = |k: Kind| shown.iter().filter(|e| e.kind == k).count();
        let mut parts = Vec::new();
        let mut add = |on: bool, count: usize, one: &str, many: &str| {
            if on && count > 0 {
                parts.push(if count == 1 { format!("1 {one}") } else { format!("{count} {many}") });
            }
        };
        add(s.finished, n(Kind::Finished), "turn finished", "turns finished");
        add(s.failed, n(Kind::Failed), "turn failed", "turns failed");
        add(s.waiting, n(Kind::Approval), "approval asked", "approvals asked");
        add(s.waiting, n(Kind::Question), "question asked", "questions asked");
        add(s.tasks, n(Kind::TaskDone), "task completed", "tasks completed");
        add(s.sessions, n(Kind::Started), "new session", "new sessions");
        let lead = if self.since == 0 { "So far".to_owned() } else { format!("Since you looked {} ago", ago(now.saturating_sub(self.since))) };
        Item {
            id: "summary".into(),
            title: if parts.is_empty() { format!("{lead}: nothing new") } else { format!("{lead}: {}", parts.join(", ")) },
            subtitle: None,
            at: (self.since > 0).then_some(self.since),
        }
    }

    /// Projects shared by more than one session get the session id's first characters.
    fn labels(&self, shown: &[&Entry]) -> Vec<(String, String)> {
        let mut seen: Vec<(String, String)> = Vec::new();
        for e in shown {
            if !seen.iter().any(|(s, _)| *s == e.session) {
                seen.push((e.session.clone(), e.project.clone()));
            }
        }
        let shared = |p: &str| seen.iter().filter(|(_, q)| q == p).count() > 1;
        seen.iter()
            .map(|(sess, p)| {
                let tag: String = sess.chars().take(4).collect();
                (sess.clone(), if shared(p) && !tag.is_empty() { format!("{p} ({tag})") } else { p.clone() })
            })
            .collect()
    }
}

fn label<'a>(labels: &'a [(String, String)], e: &'a Entry) -> &'a str {
    labels.iter().find(|(s, _)| *s == e.session).map_or(e.project.as_str(), |(_, l)| l.as_str())
}

struct Session<'a> {
    id: &'a str,
    entries: Vec<&'a Entry>,
}

/// Sessions in the order of their latest entry, newest first.
fn sessions<'a>(shown: &[&'a Entry]) -> Vec<Session<'a>> {
    let mut out: Vec<Session<'a>> = Vec::new();
    for e in shown {
        match out.iter_mut().find(|s| s.id == e.session) {
            Some(s) => s.entries.push(e),
            None => out.push(Session { id: &e.session, entries: vec![e] }),
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.entries.last().map_or(0, |e| e.at)));
    out
}

impl Session<'_> {
    fn item(&self, s: &Settings, labels: &[(String, String)]) -> Item {
        let last = self.entries.last().expect("a session has an entry");
        let state = self.entries.iter().rev().find(|e| !matches!(e.kind, Kind::TaskAdded | Kind::TaskDone)).map(|e| e.kind.phrase());
        let n = |k: Kind| self.entries.iter().filter(|e| e.kind == k).count();
        let mut parts: Vec<String> = state.into_iter().map(str::to_owned).collect();
        if s.tasks {
            let (done, added) = (n(Kind::TaskDone), n(Kind::TaskAdded));
            if done > 0 {
                parts.push(format!("{done} {} completed", if done == 1 { "task" } else { "tasks" }));
            }
            if added > 0 {
                parts.push(format!("{added} added"));
            }
        }
        if s.waiting {
            let asked = n(Kind::Approval) + n(Kind::Question);
            if asked > 0 {
                parts.push(format!("asked you {asked}×"));
            }
        }
        let latest_task = if s.tasks { self.entries.iter().rev().find(|e| e.kind == Kind::TaskAdded).and_then(|e| e.text.clone()) } else { None };
        Item {
            id: format!("s:{}", self.id),
            title: format!("{} · {}", label(labels, last), if parts.is_empty() { "active".into() } else { parts.join(" · ") }),
            subtitle: latest_task.map(|t| format!("latest task: {t}")),
            at: Some(last.at),
        }
    }
}

pub fn ago(ms: u64) -> String {
    let m = ms / 60_000;
    match m {
        0 => "under a minute".into(),
        1..=59 => format!("{m}m"),
        60..=1439 if m.is_multiple_of(60) => format!("{}h", m / 60),
        60..=1439 => format!("{}h {}m", m / 60, m % 60),
        _ => format!("{}d", m / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 1_700_000_000_000;

    fn e(at: u64, session: &str, project: &str, kind: Kind, text: Option<&str>) -> Entry {
        Entry { at, session: session.into(), project: project.into(), kind, text: text.map(Into::into) }
    }

    fn state(entries: Vec<Entry>) -> State {
        State { entries, ..State::default() }
    }

    #[test]
    fn maps_only_the_events_it_reports() {
        assert_eq!(Kind::of("stop", None), Some(Kind::Finished));
        assert_eq!(Kind::of("stop_failure", None), Some(Kind::Failed));
        assert_eq!(Kind::of("task_updated", Some("completed")), Some(Kind::TaskDone));
        assert_eq!(Kind::of("task_updated", Some("in_progress")), None);
        assert_eq!(Kind::of("tool_ran", Some("Bash")), None);
        assert_eq!(Kind::of("question_asked", None), Some(Kind::Question));
    }

    #[test]
    fn a_new_look_starts_where_the_last_ended_but_typing_keeps_it() {
        let mut st = State::default();
        st.look(T);
        assert_eq!(st.since, 0, "the first look shows everything");
        st.look(T + 10_000);
        assert_eq!(st.since, 0, "typing a query is the same look");
        st.look(T + 10_000 + LOOK_GAP_MS + 1);
        assert_eq!(st.since, T + 10_000);
    }

    #[test]
    fn the_headline_counts_only_what_happened_since() {
        let mut st = state(vec![
            e(T, "a", "vibes", Kind::Finished, None),
            e(T + 5, "a", "vibes", Kind::Finished, None),
            e(T + 6, "b", "core", Kind::Failed, None),
            e(T + 7, "b", "core", Kind::TaskDone, None),
            e(T + 8, "a", "vibes", Kind::Approval, None),
        ]);
        st.since = T;
        let v = st.view(&Settings::default(), T + 3_600_000, "");
        assert_eq!(v[0].title, "Since you looked 1h ago: 1 turn finished, 1 turn failed, 1 approval asked, 1 task completed");
        assert_eq!(v[0].at, Some(T));
    }

    #[test]
    fn nothing_new_says_so() {
        let mut st = state(vec![e(T, "a", "vibes", Kind::Finished, None)]);
        st.since = T;
        let v = st.view(&Settings::default(), T + 120_000, "");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].title, "Since you looked 2m ago: nothing new");
    }

    #[test]
    fn one_line_per_session_with_its_latest_state_and_task() {
        let st = state(vec![
            e(T, "a", "vibes", Kind::Turn, None),
            e(T + 1, "a", "vibes", Kind::TaskAdded, Some("wire the socket")),
            e(T + 2, "a", "vibes", Kind::TaskDone, None),
            e(T + 3, "a", "vibes", Kind::Finished, None),
            e(T + 4, "b", "core", Kind::Approval, None),
        ]);
        let s = Settings { timeline: false, ..Settings::default() };
        let v = st.view(&s, T + 10, "");
        assert_eq!(v.len(), 3);
        assert_eq!((v[1].title.as_str(), v[1].at), ("core · asked for approval · asked you 1×", Some(T + 4)));
        assert_eq!(v[2].title, "vibes · finished · 1 task completed · 1 added");
        assert_eq!(v[2].subtitle.as_deref(), Some("latest task: wire the socket"));
    }

    #[test]
    fn two_sessions_in_one_project_are_told_apart() {
        let st = state(vec![e(T, "aaaa1", "vibes", Kind::Finished, None), e(T + 1, "bbbb2", "vibes", Kind::Failed, None)]);
        let v = st.view(&Settings::default(), T + 10, "");
        assert_eq!(v[1].title, "vibes (bbbb) · failed");
        assert_eq!(v[3].title, "vibes (bbbb) failed");
    }

    #[test]
    fn the_timeline_is_newest_first_and_follows_the_toggles() {
        let st = state(vec![
            e(T, "a", "vibes", Kind::Started, None),
            e(T + 1, "a", "vibes", Kind::Turn, None),
            e(T + 2, "a", "vibes", Kind::TaskAdded, Some("fix it")),
            e(T + 3, "a", "vibes", Kind::Finished, None),
        ]);
        let s = Settings { per_session: false, ..Settings::default() };
        let titles: Vec<_> = st.view(&s, T + 10, "").into_iter().skip(1).map(|i| i.title).collect();
        assert_eq!(titles, ["vibes finished", "vibes new task: fix it", "vibes started"]);
        let s = Settings { per_session: false, tasks: false, sessions: false, ..Settings::default() };
        let v = st.view(&s, T + 10, "");
        assert_eq!(v.iter().skip(1).map(|i| i.title.as_str()).collect::<Vec<_>>(), ["vibes finished"]);
        assert_eq!(v[0].title, "So far: 1 turn finished");
    }

    #[test]
    fn the_query_filters_by_project_or_text() {
        let st = state(vec![e(T, "a", "vibes", Kind::Finished, None), e(T + 1, "b", "core", Kind::TaskAdded, Some("Parse WIT"))]);
        let v = st.view(&Settings::default(), T + 10, "wit");
        assert_eq!(v.iter().skip(1).map(|i| i.title.as_str()).collect::<Vec<_>>(), ["core · 1 added", "core new task: Parse WIT"]);
    }

    #[test]
    fn history_is_bounded_by_age_and_count() {
        let mut st = State::default();
        st.record(e(T, "a", "p", Kind::Finished, None), T, 7);
        st.record(e(T + 8 * DAY_MS, "a", "p", Kind::Finished, None), T + 8 * DAY_MS, 7);
        assert_eq!(st.entries.len(), 1, "an 8-day-old entry is dropped");
        for i in 0..MAX_ENTRIES as u64 + 5 {
            st.record(e(T + 8 * DAY_MS + i, "a", "p", Kind::Finished, None), T + 8 * DAY_MS + i, 7);
        }
        assert_eq!(st.entries.len(), MAX_ENTRIES);
        assert_eq!(st.entries.last().unwrap().at, T + 8 * DAY_MS + MAX_ENTRIES as u64 + 4);
    }

    #[test]
    fn agent_text_is_cleaned_and_cut() {
        assert_eq!(clean("fix\nthe\u{202E}  bug\u{1b}[31m"), "fix the bug[31m");
        assert_eq!(clean(&"x".repeat(200)).chars().count(), TEXT_MAX);
        assert_eq!(project(Some("/Users/me/code/vibes/")), Some("vibes".into()));
        assert_eq!(project(Some("/")), None);
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(ago(30_000), "under a minute");
        assert_eq!(ago(5 * 60_000), "5m");
        assert_eq!(ago(125 * 60_000), "2h 5m");
        assert_eq!(ago(3 * 1440 * 60_000), "3d");
    }
}
