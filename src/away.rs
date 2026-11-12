//! The "While you were away" sheet: what each agent did between leaving and coming back. Free of host
//! calls.

use std::collections::BTreeMap;

use crate::fleet::{Ev, Fleet, State};

/// Per tab, beyond its state: counts since you left, and the current turn's start.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tab {
    pub tools: u32,
    pub tasks_done: u32,
    pub tasks_done_before: u32,
    pub turn_started: Option<u64>,
    /// How long the last finished turn took.
    pub took: Option<u64>,
    pub ask: Option<String>,
    /// Something happened since you left.
    pub touched: bool,
}

#[derive(Debug, Default)]
pub struct Away {
    pub fleet: Fleet,
    pub tabs: BTreeMap<String, Tab>,
    pub since: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub surface: String,
    /// `✓`, `✕`, `!` or `·`.
    pub mark: char,
    pub name: String,
    pub text: String,
}

pub fn minutes(ms: u64) -> String {
    match ms / 60_000 {
        0 => format!("{}s", ms / 1000),
        m if m < 60 => format!("{m}m"),
        m => format!("{}h{:02}m", m / 60, m % 60),
    }
}

impl Away {
    /// `ask` is the waiting command, for a permission prompt.
    pub fn apply(&mut self, e: &Ev, ask: Option<&str>, task_status: Option<&str>, now: u64) {
        let change = self.fleet.apply(e, now);
        let t = self.tabs.entry(e.surface.to_owned()).or_default();
        if self.since.is_some() {
            t.touched = true;
        }
        match e.kind {
            "prompt_submit" => (t.turn_started, t.took) = (Some(now), None),
            "tool_ran" | "tool_done" | "tool_failed" | "subagent_tool" => t.tools = t.tools.saturating_add(1),
            "task_updated" if task_status == Some("completed") => t.tasks_done = t.tasks_done.saturating_add(1),
            _ => {}
        }
        if let Some(c) = change {
            if matches!(c.to, State::Done | State::Failed) {
                t.took = t.turn_started.take().map(|s| now.saturating_sub(s));
            }
            t.ask = if c.to == State::NeedsYou { ask.map(str::to_owned) } else { None };
        }
    }

    /// You stepped away: counts start over.
    pub fn leave(&mut self, now: u64) {
        self.since = Some(now);
        for t in self.tabs.values_mut() {
            (t.tools, t.tasks_done_before, t.touched) = (0, t.tasks_done, false);
        }
    }

    /// One line per agent that did something while you were away: finished first, then waiting
    /// (longest first), then still running.
    pub fn lines(&self, now: u64) -> Vec<Line> {
        let mut out: Vec<(u8, u64, Line)> = Vec::new();
        for (surface, a) in &self.fleet.agents {
            let Some(t) = self.tabs.get(surface).filter(|t| t.touched) else { continue };
            let line = |mark, text: String| Line { surface: surface.clone(), mark, name: a.name.clone(), text };
            let tasks = (t.tasks_done > t.tasks_done_before).then(|| format!("{} → {} tasks", t.tasks_done_before, t.tasks_done));
            let (rank, l) = match a.state {
                State::Done => (
                    0,
                    match (tasks, t.took) {
                        (Some(k), _) => line('✓', format!("{k} · ready to review")),
                        (None, Some(d)) => line('✓', format!("finished · Done in {}", minutes(d))),
                        (None, None) => line('✓', "finished".into()),
                    },
                ),
                State::Failed => (0, line('✕', t.took.map_or_else(|| "failed".into(), |d| format!("failed after {}", minutes(d))))),
                State::NeedsYou => {
                    let wait = format!("waiting {}", minutes(now.saturating_sub(a.since)));
                    (1, line('!', t.ask.as_ref().map_or(wait.clone(), |q| format!("{wait} · {q}?"))))
                }
                State::Running => (2, line('·', format!("still running · {} tools", t.tools))),
            };
            out.push((rank, if rank == 1 { a.since } else { 0 }, l));
        }
        out.sort_by_key(|x| (x.0, x.1));
        out.into_iter().map(|(_, _, l)| l).collect()
    }

    /// The agent to start with: the one that has waited longest.
    pub fn start_with(&self) -> Option<(&str, &str)> {
        self.fleet.longest_waiting().map(|(s, a)| (s, a.name.as_str()))
    }
}

/// `✓ docs   finished · Done in 4m`, names padded to one column.
pub fn render(lines: &[Line]) -> Vec<String> {
    let w = lines.iter().map(|l| l.name.chars().count()).max().unwrap_or(0);
    lines.iter().map(|l| format!("{} {:<w$}  {}", l.mark, l.name, l.text)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev<'a>(surface: &'a str, kind: &'a str, cwd: &'a str) -> Ev<'a> {
        Ev { surface, kind, cwd: Some(cwd), ..Ev::default() }
    }

    #[test]
    fn the_sheet_reads_like_the_mockup() {
        let mut a = Away::default();
        let m = 60_000;
        for (s, cwd) in [("d", "/w/docs"), ("w", "/w/web"), ("a", "/w/api"), ("i", "/w/infra"), ("q", "/w/quiet")] {
            a.apply(&ev(s, "prompt_submit", cwd), None, None, 0);
        }
        a.apply(&ev("q", "stop", "/w/quiet"), None, None, m);
        for _ in 0..3 {
            a.apply(&ev("w", "task_updated", "/w/web"), None, Some("completed"), m);
        }
        a.leave(2 * m);
        a.apply(&ev("d", "stop", "/w/docs"), None, None, 4 * m);
        for _ in 0..2 {
            a.apply(&ev("w", "task_updated", "/w/web"), None, Some("completed"), 5 * m);
        }
        a.apply(&ev("w", "task_updated", "/w/web"), None, Some("in_progress"), 5 * m);
        a.apply(&ev("w", "stop", "/w/web"), None, None, 6 * m);
        a.apply(&ev("a", "permission_seen", "/w/api"), Some("git push"), None, 13 * m);
        for _ in 0..41 {
            a.apply(&ev("i", "tool_ran", "/w/infra"), None, None, 14 * m);
        }
        assert_eq!(
            render(&a.lines(25 * m)),
            [
                "✓ docs   finished · Done in 4m",
                "✓ web    3 → 5 tasks · ready to review",
                "! api    waiting 12m · git push?",
                "· infra  still running · 41 tools",
            ],
            "quiet did nothing while you were away"
        );
        assert_eq!(a.start_with(), Some(("a", "api")));
    }

    #[test]
    fn times() {
        assert_eq!([30_000, 240_000, 3_900_000].map(minutes), ["30s", "4m", "1h05m"]);
    }
}
