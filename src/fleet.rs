//! Each tab's agent state, followed from its hook events the way Haunt's sidebar does. Free of host
//! calls so it tests natively.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    NeedsYou,
    Done,
    Failed,
}

/// The fields of an agent event this module reads.
#[derive(Debug, Clone, Default)]
pub struct Ev<'a> {
    pub surface: &'a str,
    pub kind: &'a str,
    pub cwd: Option<&'a str>,
    /// `notification_type` for `notification`, else the event's summary.
    pub summary: Option<&'a str>,
    pub subagent: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    pub name: String,
    pub state: State,
    /// When it entered `state`, in ms.
    pub since: u64,
    pub tools: u32,
    pub subagents: u32,
}

/// What an event did to one tab's state, for plugins that react to changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub from: Option<State>,
    pub to: State,
}

#[derive(Debug, Clone, Default)]
pub struct Fleet {
    pub agents: BTreeMap<String, Agent>,
}

/// The row name Haunt shows: the last part of the cwd.
pub fn name_of(cwd: Option<&str>) -> Option<String> {
    let c = cwd?.trim_end_matches('/');
    let n = c.rsplit('/').next().filter(|s| !s.is_empty())?;
    let n: String = n.chars().filter(|c| !c.is_control() && !matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')).take(48).collect();
    (!n.is_empty()).then_some(n)
}

impl Fleet {
    pub fn apply(&mut self, e: &Ev, now: u64) -> Option<Change> {
        let next = match e.kind {
            "session_start" | "prompt_submit" | "tool_start" | "tool_ran" | "tool_done" | "tool_failed" | "subagent_spawn" | "subagent_start" | "subagent_tool" | "permission_replied" | "tool_complete" => Some(State::Running),
            "permission_seen" | "permission_request" | "question_asked" | "agent_needs_input" => Some(State::NeedsYou),
            "notification" if e.summary == Some("permission_prompt") => Some(State::NeedsYou),
            "stop" => Some(State::Done),
            "stop_failure" => Some(State::Failed),
            _ => None,
        };
        let a = self.agents.entry(e.surface.to_owned()).or_insert_with(|| Agent { name: "agent".into(), state: State::Done, since: now, tools: 0, subagents: 0 });
        let fresh = a.since == now && a.tools == 0 && a.subagents == 0 && a.name == "agent";
        if let Some(n) = name_of(e.cwd) {
            a.name = n;
        }
        if e.kind == "prompt_submit" {
            (a.tools, a.subagents) = (0, 0);
        }
        if matches!(e.kind, "tool_ran" | "tool_done" | "tool_failed" | "subagent_tool") {
            a.tools = a.tools.saturating_add(1);
        }
        if e.kind == "subagent_spawn" {
            a.subagents = a.subagents.saturating_add(1);
        }
        let mut next = next?;
        // Tool events while a prompt is up are the main agent's other calls or a subagent's; the
        // prompt stays until its own call runs, which a plugin can't match, so only the main thread's
        // finished call ends it.
        if a.state == State::NeedsYou && next == State::Running && (e.subagent || e.kind == "tool_start") {
            next = State::NeedsYou;
        }
        let from = (!fresh).then_some(a.state);
        if from == Some(next) {
            return None;
        }
        (a.state, a.since) = (next, now);
        Some(Change { from, to: next })
    }

    pub fn remove(&mut self, surface: &str) {
        self.agents.remove(surface);
    }

    pub fn count(&self, s: State) -> usize {
        self.agents.values().filter(|a| a.state == s).count()
    }

    /// The tab that has waited longest for you.
    pub fn longest_waiting(&self) -> Option<(&str, &Agent)> {
        self.agents.iter().filter(|(_, a)| a.state == State::NeedsYou).min_by_key(|(_, a)| a.since).map(|(s, a)| (s.as_str(), a))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev<'a>(surface: &'a str, kind: &'a str) -> Ev<'a> {
        Ev { surface, kind, cwd: Some("/w/api"), ..Ev::default() }
    }

    #[test]
    fn follows_a_turn() {
        let mut f = Fleet::default();
        assert_eq!(f.apply(&ev("s", "prompt_submit"), 1), Some(Change { from: None, to: State::Running }));
        assert_eq!(f.agents["s"].name, "api");
        assert_eq!(f.apply(&ev("s", "tool_ran"), 2), None);
        assert_eq!(f.apply(&ev("s", "permission_seen"), 3), Some(Change { from: Some(State::Running), to: State::NeedsYou }));
        assert_eq!(f.apply(&ev("s", "tool_start"), 4), None, "a start doesn't answer the prompt");
        assert_eq!(f.apply(&Ev { subagent: true, ..ev("s", "tool_ran") }, 5), None, "nor does a subagent's call");
        assert_eq!(f.apply(&ev("s", "tool_ran"), 6).unwrap().to, State::Running);
        assert_eq!(f.apply(&ev("s", "stop"), 7).unwrap().to, State::Done);
        assert_eq!((f.agents["s"].tools, f.agents["s"].since), (3, 7));
    }

    #[test]
    fn permission_notifications_and_failures() {
        let mut f = Fleet::default();
        f.apply(&ev("s", "prompt_submit"), 1);
        assert_eq!(f.apply(&Ev { summary: Some("idle_prompt"), ..ev("s", "notification") }, 2), None);
        assert_eq!(f.apply(&Ev { summary: Some("permission_prompt"), ..ev("s", "notification") }, 3).unwrap().to, State::NeedsYou);
        assert_eq!(f.apply(&ev("s", "stop_failure"), 4).unwrap().to, State::Failed);
    }

    #[test]
    fn counts_and_longest_wait() {
        let mut f = Fleet::default();
        for (s, t) in [("a", 5), ("b", 2), ("c", 9)] {
            f.apply(&ev(s, "permission_seen"), t);
        }
        f.apply(&ev("c", "tool_ran"), 10);
        assert_eq!((f.count(State::NeedsYou), f.count(State::Running)), (2, 1));
        assert_eq!(f.longest_waiting().unwrap().0, "b");
        f.remove("b");
        assert_eq!(f.longest_waiting().unwrap().0, "a");
    }

    #[test]
    fn names_are_cleaned() {
        assert_eq!(name_of(Some("/w/we\u{202e}b/")).as_deref(), Some("web"));
        assert_eq!(name_of(Some("/")), None);
        assert_eq!(name_of(None), None);
    }
}
