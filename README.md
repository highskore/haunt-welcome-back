<div align="center">

<img src="docs/icon.png" width="96" alt="">

# Welcome back

**What your agents did while you were away, waiting for you when you're back.**

[![Haunt plugin](https://img.shields.io/badge/Haunt-plugin-D97757)](https://github.com/highskore/haunt-hackathon)
![Plugin API 0.5](https://img.shields.io/badge/plugin%20API-0.5-111)
![WASM component](https://img.shields.io/badge/WASM-component-654ff0?logo=webassembly&logoColor=white)
![MIT licence](https://img.shields.io/badge/licence-MIT-6b7280)

</div>

## The sheet

Step away for 20 minutes or more (screen locked, asleep, or idle for Haunt's away time) and a sheet greets you when you come back and switch to Haunt (it's dropped if you start typing first, or don't come within 10 minutes):

```
While you were away · 23 min
✓ docs   finished · Done in 4m
✓ web    3 → 5 tasks · ready to review
! api    waiting 12m · git push?
· infra  still running · 41 tools
[Start with api]  [Dismiss]
```

One line per agent that did something while you were gone: finished ones first, then the ones waiting on you (longest first, with the command they want to run), then the ones still running with their tool calls since you left. **Start with api** brings up the tab of the agent that has waited longest. Nothing happened, no sheet.

## The `back` mode

Type `back ` in the ⌘K palette for the longer history:

```
Since you looked 1h 12m ago: 3 turns finished, 1 turn failed, 2 approvals asked, 7 tasks completed
vibes · finished · 4 tasks completed · 5 added          latest task: Wire the socket
core (a1b2) · asked for approval · asked you 2×
core (9f0e) · failed
vibes finished
core (a1b2) asked for approval
...
```

- **The headline** counts what happened since you last opened `back`. Reopening the palette within 90 s counts as the same look, so typing a query doesn't reset it. The first look shows everything kept.
- **One line per session**, newest first: the session's project (the last part of its folder, with the session id's first characters when two sessions share one), its latest state, the tasks it completed and added, and how often it asked you. The time column shows its last event.
- **The timeline**: every event, newest first.
- Type after `back ` to filter by project or task text.

Everything comes from the agents' own hook events, as they arrive: nothing is estimated or sampled. Events that came without the tab's nonce (anything printed in the terminal can forge one) are left out unless `unverified = true`. Task text is the agent's, so it is kept on one line, stripped of control and invisible characters and cut to 80 characters.

Enter on a `back` line doesn't jump to the tab yet.

## Settings

In `~/.haunt/config.toml`:

```toml
[plugins."dev.haunt.welcome-back"]
sheet = true         # the "While you were away" sheet
min_away_min = 20    # minutes away before it shows
finished = true      # turns that finished
failed = true        # turns that failed
waiting = true       # approvals and questions an agent asked you
tasks = true         # tasks added and completed
sessions = true      # new agent sessions
per_session = true   # one summary line per session
timeline = true      # every event, newest first
unverified = false   # also count events without the tab's nonce
keep_days = 7        # days of history to keep (at most 1000 events)
```

## Capabilities

| capability | why |
|---|---|
| `events`: session start, prompt, task, subagent, tool, permission request, question, stop, unknown | sessions starting, turns starting, finishing or failing (`stop_failure` arrives as `agent.unknown`), tasks, tool calls counted for the sheet, and the agent asking you |
| `event_details` | the waiting command in the sheet ("git push?"); haunt asks in red, since commands can hold secrets. Only the sheet shows it; the history keeps no command. |
| `away` | when you step away and come back |
| `panel` | the sheet |

No files, pty, notifications or decision log. The history (at most 1000 events) stays in the plugin's own storage.

## Build, test, install

```sh
cargo test                                     # native unit tests
cargo build --release --target wasm32-wasip2   # or ./build.sh
haunt plugin install .
```

`e2e/welcome_back.rs` runs the plugin in a real Haunt core: copy it into a haunt checkout's `cli/tests/` and run `HAUNT_PLUGINS_DIR=<this repo's parent> cargo test -p haunt-cli --features plugins --test welcome_back`.

It needs Haunt with `haunt:plugin@0.5` (`api = "0.5"`) and its `on-away-changed` export. `wit/haunt-plugin.wit` is a copy of Haunt's, pinned at that version.

## Licence

MIT, see `LICENSE`.
