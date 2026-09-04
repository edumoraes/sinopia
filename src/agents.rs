//! The agents running on this machine: where each one is working, and
//! how to reach it. Three backends answer, and parsing what they say is
//! pure — the shell runs the commands, this decides what they meant.
//!
//! The window manager is deliberately not one of them. One terminal
//! window here holds a multiplexer with a dozen shells and agents in
//! different projects, so a focused window cannot answer which agent is
//! meant. The list can.

use std::process::Command;

/// The process names this looks for. A name it does not know is not an
/// agent, and guessing would put a text editor in the list.
pub const KNOWN: [&str; 5] = ["claude", "codex", "opencode", "crush", "gemini"];

/// How a running agent can be handed a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reach {
    /// A herdr pane, by its id.
    Herdr(String),
    /// A tmux pane, by its id (`%0`).
    Tmux(String),
    /// Running, and nothing here knows how to talk to it. The files can
    /// still land in its folder, which is the half nobody does by hand.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    /// The process's own name: `claude`, `codex`, …
    pub kind: String,
    /// The directory it is working in — where the export lands.
    pub cwd: String,
    pub reach: Reach,
    pub focused: bool,
    /// What the backend says it is doing, when it says anything.
    pub status: Option<String>,
}

impl Agent {
    pub fn at(kind: &str, cwd: &str, reach: Reach) -> Agent {
        Agent {
            kind: kind.to_owned(),
            cwd: cwd.to_owned(),
            reach,
            focused: false,
            status: None,
        }
    }

    /// What the panel writes on its row.
    pub fn label(&self) -> String {
        format!("{} · {}", self.kind, self.cwd)
    }

    /// Whether a prompt can be handed to it.
    pub fn reachable(&self) -> bool {
        self.reach != Reach::None
    }
}

/// `herdr agent list`, which answers one JSON object with the agents
/// under `result.agents`. Anything it says that cannot be read is no
/// agents rather than an error: a missing herdr and a broken herdr are
/// the same thing to a board.
pub fn parse_herdr(out: &str) -> Vec<Agent> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(out) else {
        return Vec::new();
    };
    let Some(list) = v["result"]["agents"].as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|a| {
            let kind = a["agent"].as_str()?;
            let cwd = a["cwd"].as_str()?;
            let pane = a["pane_id"].as_str()?;
            Some(Agent {
                focused: a["focused"].as_bool().unwrap_or(false),
                status: a["agent_status"].as_str().map(str::to_owned),
                ..Agent::at(kind, cwd, Reach::Herdr(pane.to_owned()))
            })
        })
        .collect()
}

/// The format string [`list`] asks tmux for.
pub const TMUX_FORMAT: &str =
    "#{pane_current_command}|#{pane_id}|#{pane_active}|#{pane_current_path}";

/// tmux's answer to [`TMUX_FORMAT`], one pane a line. Only the panes
/// running something [`KNOWN`] are agents.
pub fn parse_tmux(out: &str) -> Vec<Agent> {
    out.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '|');
            let kind = parts.next()?;
            let pane = parts.next()?;
            let active = parts.next()?;
            let cwd = parts.next()?;
            if !KNOWN.contains(&kind) {
                return None;
            }
            Some(Agent {
                focused: active == "1",
                ..Agent::at(kind, cwd, Reach::Tmux(pane.to_owned()))
            })
        })
        .collect()
}

/// A `/proc` walk's `(comm, cwd)` rows. It finds every agent either
/// multiplexer knows about and any running under neither, and knows how
/// to reach none of them.
pub fn parse_proc(rows: &[(String, String)]) -> Vec<Agent> {
    rows.iter()
        .filter(|(comm, _)| KNOWN.contains(&comm.as_str()))
        .map(|(comm, cwd)| Agent::at(comm, cwd, Reach::None))
        .collect()
}

/// One list out of the three: an agent is its directory, the entry that
/// can be reached wins, and the focused one is put first because it is
/// the one the person just came from.
pub fn merge(all: Vec<Agent>) -> Vec<Agent> {
    let mut out: Vec<Agent> = Vec::new();
    for a in all {
        match out.iter_mut().find(|b| b.cwd == a.cwd && b.kind == a.kind) {
            Some(b) => {
                if !b.reachable() && a.reachable() {
                    b.reach = a.reach;
                }
                b.focused |= a.focused;
                b.status = b.status.take().or(a.status);
            }
            None => out.push(a),
        }
    }
    out.sort_by_key(|a| !a.focused);
    out
}

/// What is running now. Every backend that is not there answers nothing,
/// which is the same as a backend with nothing to say.
pub fn list() -> Vec<Agent> {
    let mut all = Vec::new();
    if let Some(out) = run("herdr", &["agent", "list"]) {
        all.extend(parse_herdr(&out));
    }
    if let Some(out) = run("tmux", &["list-panes", "-a", "-F", TMUX_FORMAT]) {
        all.extend(parse_tmux(&out));
    }
    all.extend(parse_proc(&scan_proc()));
    merge(all)
}

/// A command's stdout, or nothing at all: a backend that is not
/// installed, that fails, or that says something unreadable is a backend
/// with no agents.
fn run(bin: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(bin).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Every process's name and working directory, for the ones this user
/// may read. A process that ends between the two reads is simply not in
/// the list.
fn scan_proc() -> Vec<(String, String)> {
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    dir.flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
        })
        .filter_map(|e| {
            let comm = std::fs::read_to_string(e.path().join("comm")).ok()?;
            let cwd = std::fs::read_link(e.path().join("cwd")).ok()?;
            Some((comm.trim().to_owned(), cwd.to_string_lossy().into_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HERDR: &str = r#"{"id":"cli:agent:list","result":{"agents":[
      {"agent":"claude","agent_status":"idle","cwd":"/home/e/Work/a","focused":false,"pane_id":"w1:p1","terminal_title":"Claude Code"},
      {"agent":"claude","agent_status":"working","cwd":"/home/e/Work/b","focused":true,"pane_id":"wA:p1","terminal_title":"Claude Code"}
    ],"type":"agent_list"}}"#;

    #[test]
    fn herdr_says_where_each_agent_is_and_which_one_is_focused() {
        let found = parse_herdr(HERDR);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].kind, "claude");
        assert_eq!(found[0].cwd, "/home/e/Work/a");
        assert_eq!(found[0].reach, Reach::Herdr("w1:p1".into()));
        assert!(!found[0].focused);
        assert!(found[1].focused);
        assert_eq!(found[1].status.as_deref(), Some("working"));
    }

    #[test]
    fn herdr_saying_nothing_it_can_parse_is_no_agents_and_not_a_crash() {
        assert!(parse_herdr("").is_empty());
        assert!(parse_herdr("not json").is_empty());
        assert!(parse_herdr(r#"{"result":{}}"#).is_empty());
    }

    #[test]
    fn tmux_lists_only_the_panes_running_something_we_know() {
        let out = "claude|%0|1|/home/e/Work/a\nbash|%1|0|/home/e\nvim|%2|1|/home/e/Work/b\n";
        let found = parse_tmux(out);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, "claude");
        assert_eq!(found[0].reach, Reach::Tmux("%0".into()));
        assert!(found[0].focused, "the active pane is the focused one");
    }

    #[test]
    fn tmux_saying_nothing_is_no_agents() {
        assert!(parse_tmux("").is_empty());
        assert!(parse_tmux("garbage\n").is_empty());
    }

    #[test]
    fn a_proc_scan_finds_agents_that_nothing_can_reach() {
        let rows = [
            ("claude".to_owned(), "/home/e/Work/c".to_owned()),
            ("bash".to_owned(), "/home/e".to_owned()),
        ];
        let found = parse_proc(&rows);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].reach, Reach::None);
    }

    #[test]
    fn an_agent_a_multiplexer_already_claimed_is_not_listed_twice() {
        // The /proc scan sees every agent, including the ones inside
        // herdr and tmux. The one that can be reached wins.
        let all = vec![
            Agent::at("claude", "/home/e/Work/a", Reach::Herdr("w1:p1".into())),
            Agent::at("claude", "/home/e/Work/a", Reach::None),
            Agent::at("claude", "/home/e/Work/c", Reach::None),
        ];
        let merged = merge(all);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].reach, Reach::Herdr("w1:p1".into()));
        assert_eq!(merged[1].reach, Reach::None);
    }

    #[test]
    fn the_focused_agent_is_listed_first() {
        let all = vec![
            Agent::at("claude", "/home/e/Work/a", Reach::None),
            Agent {
                focused: true,
                ..Agent::at("claude", "/home/e/Work/b", Reach::Tmux("%0".into()))
            },
        ];
        assert_eq!(merge(all)[0].cwd, "/home/e/Work/b");
    }

    #[test]
    fn an_agent_is_labelled_by_what_it_is_and_where_it_is_working() {
        let a = Agent::at("claude", "/home/e/Work/board", Reach::None);
        assert_eq!(a.label(), "claude · /home/e/Work/board");
    }
}
