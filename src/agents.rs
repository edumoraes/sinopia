//! The agents running on this machine: where each one is working, and
//! how to reach it. Three backends answer, and parsing what they say is
//! pure — the shell runs the commands, this decides what they meant.
//!
//! The window manager is deliberately not one of them. One terminal
//! window here holds a multiplexer with a dozen shells and agents in
//! different projects, so a focused window cannot answer which agent is
//! meant. The list can.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context as _;

use crate::skills;

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

    /// What the people who made it call it. The kind is the process's
    /// name, which is what `/proc` and herdr answer with, and nobody
    /// calls Claude Code `claude` in a sentence. One this build has not
    /// met keeps the name it came with.
    pub fn name(&self) -> &str {
        match self.kind.as_str() {
            "claude" => "Claude Code",
            "codex" => "Codex",
            "opencode" => "OpenCode",
            "crush" => "Crush",
            "gemini" => "Gemini CLI",
            other => other,
        }
    }

    /// Which cell of the logo sheet carries its maker's mark: the sheet
    /// is laid out in [`KNOWN`]'s order. One herdr knows and this build
    /// does not has none.
    pub fn mark(&self) -> Option<usize> {
        KNOWN.iter().position(|k| *k == self.kind)
    }

    /// Whether a prompt can be handed to it.
    pub fn reachable(&self) -> bool {
        self.reach != Reach::None
    }
}

/// What each row says for its folder: the last directory of where the
/// agent is working — that is the project, and the path above it is the
/// person's home on every row alike — and only as many directories above
/// it as it takes for two different places not to read the same. Two
/// agents in one place are one place, and say it alike.
pub fn folders(agents: &[Agent]) -> Vec<String> {
    let parts: Vec<Vec<&str>> = agents
        .iter()
        .map(|a| a.cwd.split('/').filter(|p| !p.is_empty()).collect())
        .collect();
    let tail = |p: &[&str], depth: usize| match p.len() {
        0 => "/".to_owned(),
        n => p[n - depth.min(n)..].join("/"),
    };
    let mut depth = vec![1usize; agents.len()];
    loop {
        let shown: Vec<String> = parts.iter().zip(&depth).map(|(p, &d)| tail(p, d)).collect();
        let mut grew = false;
        for i in 0..agents.len() {
            let clash =
                (0..agents.len()).any(|j| j != i && shown[j] == shown[i] && parts[j] != parts[i]);
            if clash && depth[i] < parts[i].len() {
                depth[i] += 1;
                grew = true;
            }
        }
        if !grew {
            return shown;
        }
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

/// Where `agent` stands in `list`: the entry that is the same agent — the
/// same kind in the same directory, which is what [`merge`] takes an agent
/// to be — wherever the list now puts it.
pub fn find(list: &[Agent], agent: &Agent) -> Option<usize> {
    list.iter()
        .position(|a| a.kind == agent.kind && a.cwd == agent.cwd)
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

/// The most an instruction may carry. A prompt is a direction, not a
/// document — but a box that scrolls past twenty lines is room for a
/// pasted trace or a list, and the cap is well past that.
pub const PROMPT_MAX: usize = 16 * 1024;

/// The one new power this feature has is writing bytes into a live
/// terminal, so the instruction is printable characters and newlines and
/// nothing else: no ESC, no carriage return, not even a tab. It goes in
/// as a bracketed paste, where a newline is a line of the same message
/// and never an early submit; the rest of C0 has no such frame around
/// it. Anything else is refused rather than stripped — an instruction the
/// person cannot see being altered is worse than one that does not go.
pub fn sanitize(line: &str) -> anyhow::Result<String> {
    let line = line.trim();
    anyhow::ensure!(!line.is_empty(), "the instruction is empty");
    anyhow::ensure!(
        line.len() <= PROMPT_MAX,
        "the instruction is longer than {PROMPT_MAX} bytes"
    );
    if let Some(c) = line.chars().find(|&c| c.is_control() && c != '\n') {
        anyhow::bail!(
            "the instruction holds a control character (U+{:04X})",
            c as u32
        );
    }
    Ok(line.to_owned())
}

/// What the agent is handed: the person's own line, then the files under
/// a heading that says what they are. The board's own text is nowhere in
/// it — the document is inventory, the instruction is a deliberate act
/// (§9.4).
pub fn prompt(line: &str, files: &[String]) -> String {
    let mut out = format!("{line}\n\nDiagram exported from the board:\n");
    for f in files {
        out.push_str(&format!("  {f}\n"));
    }
    out
}

/// The paths as the agent will type them: it is running in `cwd` and the
/// files were written under it, so an absolute path would only say where
/// the person's home is.
pub fn relative(files: &[PathBuf], cwd: &Path) -> Vec<String> {
    files
        .iter()
        .map(|f| {
            f.strip_prefix(cwd)
                .unwrap_or(f)
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// What of `prompt` has to arrive typed, and what is pasted after it.
/// Claude Code reads a skill's call only where it was typed: a paste of
/// more than three lines is a placeholder, and a `/` that arrives inside
/// one is text — the skill never runs. So for it the call a prompt opens
/// with goes in as keys, with the space that ends it, and the rest as
/// the paste it always was, which becomes the call's arguments. Every
/// other harness reads a call inside a paste, and is pasted the whole.
pub fn typed_call(kind: &str, prompt: &str) -> (Option<String>, String) {
    let typed = skills::harness(kind).is_some_and(|h| h.typed);
    if typed && let Some(rest) = prompt.strip_prefix('/') {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let name = &rest[..end];
        if skills::callable(name) {
            return (
                Some(format!("/{name} ")),
                rest[end..].trim_start().to_owned(),
            );
        }
    }
    (None, prompt.to_owned())
}

/// What herdr's `agent list` says the agent in `pane` is doing — `idle`,
/// `working`, `blocked`, `done` or `unknown` — or nothing, for a pane it
/// does not list or an answer that will not parse.
pub fn status_of(list: &str, pane: &str) -> Option<String> {
    parse_herdr(list)
        .into_iter()
        .find(|a| a.reach == Reach::Herdr(pane.to_owned()))
        .and_then(|a| a.status)
}

/// Whether a call may be typed at an agent doing `status`. herdr's
/// `agent prompt` refuses a blocked agent before any input is sent, but
/// the keys a call is typed with go ahead of it and ask nothing — into
/// the question the agent is holding open, where a digit in a name can
/// pick one of its options. So a blocked agent is not typed at, and
/// neither is one herdr cannot say anything about.
pub fn free(status: Option<&str>) -> anyhow::Result<()> {
    match status {
        Some("blocked") => anyhow::bail!("the agent is waiting on an answer; nothing was sent"),
        Some(_) => Ok(()),
        None => anyhow::bail!("herdr could not say whether the agent is free; nothing was sent"),
    }
}

/// How long the paste is given before the Enter that submits it. Gemini
/// CLI takes an Enter within 40 ms of a paste for a newline in it; herdr
/// waits this long before its own, and tmux is made to.
const SUBMIT_AFTER: std::time::Duration = std::time::Duration::from_millis(300);

/// Hands `text` to a running agent. The text never becomes part of a
/// shell command: herdr takes it as an argument and tmux takes it
/// through a buffer on stdin, so a line holding `$(…)` or `;` has
/// nowhere to run. A skill's call that has to be typed goes in first, as
/// literal keys — and only ever as a name [`skills::callable`] passed.
pub fn send(agent: &Agent, text: &str) -> anyhow::Result<()> {
    let (typed, pasted) = typed_call(&agent.kind, text);
    match &agent.reach {
        Reach::None => anyhow::bail!("nothing here knows how to reach that agent"),
        Reach::Herdr(pane) => {
            if let Some(typed) = &typed {
                // Asked as late as it can be: the dialog's own list may be
                // minutes old.
                let listed = run("herdr", &["agent", "list"]).unwrap_or_default();
                free(status_of(&listed, pane).as_deref())?;
                // Raw text, never bracketed, and no Enter.
                let out = Command::new("herdr")
                    .args(["pane", "send-text", pane, typed])
                    .output()
                    .context("running herdr pane send-text")?;
                anyhow::ensure!(
                    out.status.success(),
                    "herdr would not type the skill's call: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            let out = Command::new("herdr")
                .args(["agent", "prompt", pane, &pasted])
                .output()
                .context("running herdr agent prompt")?;
            anyhow::ensure!(
                out.status.success(),
                "herdr refused the prompt: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
            Ok(())
        }
        Reach::Tmux(pane) => {
            if let Some(typed) = &typed {
                let keyed = Command::new("tmux")
                    .args(["send-keys", "-t", pane, "-l", typed])
                    .status()
                    .context("running tmux send-keys")?;
                anyhow::ensure!(keyed.success(), "tmux would not type into {pane}");
            }
            let text = pasted.as_str();
            let buffer = "sinopia";
            // load-buffer reads the text from stdin, so it is never a
            // word on a command line.
            let mut child = Command::new("tmux")
                .args(["load-buffer", "-b", buffer, "-"])
                .stdin(std::process::Stdio::piped())
                .spawn()
                .context("running tmux load-buffer")?;
            {
                use std::io::Write as _;
                let mut stdin = child.stdin.take().context("tmux took no stdin")?;
                stdin.write_all(text.as_bytes())?;
            }
            anyhow::ensure!(child.wait()?.success(), "tmux would not take the buffer");
            // -p is what wraps it in a bracketed paste when the TUI has
            // asked for one. Without it a multi-line prompt submits at
            // its first newline and becomes several turns.
            let pasted = Command::new("tmux")
                .args(["paste-buffer", "-p", "-b", buffer, "-t", pane])
                .status()
                .context("running tmux paste-buffer")?;
            anyhow::ensure!(pasted.success(), "tmux would not paste into {pane}");
            std::thread::sleep(SUBMIT_AFTER);
            let sent = Command::new("tmux")
                .args(["send-keys", "-t", pane, "Enter"])
                .status()
                .context("running tmux send-keys")?;
            anyhow::ensure!(sent.success(), "tmux would not submit in {pane}");
            Ok(())
        }
    }
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
    fn an_agent_is_found_again_in_a_list_that_came_back_in_another_order() {
        // Coming back to the window lists the agents again, the focused
        // one first: the one the dialog was aimed at has to be found by
        // what it is, not by where it stood.
        let a = Agent::at("claude", "/w/a", Reach::Herdr("p1".into()));
        let b = Agent::at("codex", "/w/b", Reach::Tmux("%1".into()));
        let again = [
            b.clone(),
            Agent {
                focused: true,
                ..a.clone()
            },
        ];
        assert_eq!(find(&again, &a), Some(1));
        assert_eq!(find(&again, &b), Some(0));
        assert_eq!(
            find(&again, &Agent::at("claude", "/w/c", Reach::None)),
            None
        );
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
    fn an_agent_is_called_by_the_name_its_makers_gave_it() {
        let called = |kind| Agent::at(kind, "/w/a", Reach::None).name().to_owned();
        assert_eq!(called("claude"), "Claude Code");
        assert_eq!(called("codex"), "Codex");
        assert_eq!(called("opencode"), "OpenCode");
        assert_eq!(called("crush"), "Crush");
        assert_eq!(called("gemini"), "Gemini CLI");
    }

    #[test]
    fn each_known_agent_has_a_cell_of_the_logo_sheet_in_the_order_it_is_known() {
        let cell = |kind| Agent::at(kind, "/w/a", Reach::None).mark();
        for (i, kind) in KNOWN.iter().enumerate() {
            assert_eq!(cell(kind), Some(i));
        }
        assert_eq!(cell("pi"), None, "a mark nobody drew is no mark");
    }

    #[test]
    fn an_agent_this_build_has_not_met_is_called_what_herdr_calls_it() {
        // herdr names agents of its own, and one nobody here has heard
        // of is still an agent worth listing.
        assert_eq!(
            Agent::at("pi", "/w/a", Reach::Herdr("p".into())).name(),
            "pi"
        );
    }

    #[test]
    fn an_agent_says_where_it_is_by_the_last_directory_of_its_path() {
        let at = |cwd| folders(&[Agent::at("claude", cwd, Reach::None)]);
        assert_eq!(at("/home/e/Work/board"), ["board"]);
        assert_eq!(at("/home/e/Work/board/"), ["board"]);
        assert_eq!(at("/"), ["/"]);
    }

    #[test]
    fn two_folders_of_one_name_are_told_apart_by_what_is_above_them() {
        // The last directory is the whole of what a row says, so two
        // projects both called `app` would be two rows saying the same
        // thing. Each goes up one directory at a time until it is not.
        let agents = [
            Agent::at("claude", "/home/e/Work/app", Reach::None),
            Agent::at("codex", "/home/e/Play/app", Reach::None),
            Agent::at("claude", "/home/e/Work/board", Reach::None),
        ];
        assert_eq!(folders(&agents), ["Work/app", "Play/app", "board"]);
    }

    #[test]
    fn one_folder_seen_twice_is_one_folder() {
        // Two agents in the same directory are not a clash: the folder
        // is the same place, and saying more would say nothing.
        let agents = [
            Agent::at("claude", "/home/e/Work/board", Reach::None),
            Agent::at("codex", "/home/e/Work/board", Reach::None),
        ];
        assert_eq!(folders(&agents), ["board", "board"]);
    }

    #[test]
    fn herdr_says_what_the_agent_in_a_pane_is_doing() {
        let list = r#"{"result":{"agents":[
          {"agent":"claude","pane_id":"w1:p1","agent_status":"blocked","cwd":"/a"},
          {"agent":"codex","pane_id":"w2:p1","agent_status":"idle","cwd":"/b"}
        ]}}"#;
        assert_eq!(status_of(list, "w1:p1").as_deref(), Some("blocked"));
        assert_eq!(status_of(list, "w2:p1").as_deref(), Some("idle"));
        assert_eq!(status_of(list, "w9:p9"), None, "a pane herdr does not list");
        assert_eq!(status_of("not json", "w1:p1"), None);
    }

    #[test]
    fn nothing_is_typed_at_an_agent_waiting_on_an_answer() {
        // A blocked agent is holding a question open, and the call typed
        // into it would be its answer. An agent herdr cannot say anything
        // about is not typed at either.
        assert!(free(Some("blocked")).is_err());
        assert!(free(None).is_err());
        for status in ["idle", "working", "done", "unknown"] {
            assert!(free(Some(status)).is_ok(), "{status}");
        }
    }

    #[test]
    fn claude_code_gets_the_skill_it_is_called_with_typed_and_the_rest_pasted() {
        let prompt = "/frontend-design make it calm\n\nDiagram exported from the board:\n";
        let (typed, pasted) = typed_call("claude", prompt);
        assert_eq!(typed.as_deref(), Some("/frontend-design "));
        assert_eq!(pasted, "make it calm\n\nDiagram exported from the board:\n");
        let (typed, pasted) = typed_call("claude", "/review\nthe flow");
        assert_eq!(typed.as_deref(), Some("/review "));
        assert_eq!(
            pasted, "the flow",
            "a break after the call is the space it needs"
        );
    }

    #[test]
    fn a_prompt_that_calls_no_skill_is_pasted_whole() {
        for (kind, prompt) in [
            ("claude", "draw it\n"),
            ("claude", "/ spaced\n"),
            ("claude", "/a;b rest\n"),
            // These read a call inside a paste, so nothing is typed.
            ("codex", "$review rest\n"),
            ("opencode", "/review rest\n"),
            ("gemini", "/review rest\n"),
            ("pi", "/review rest\n"),
        ] {
            assert_eq!(
                typed_call(kind, prompt),
                (None, prompt.to_owned()),
                "{kind}: {prompt:?}"
            );
        }
    }

    #[test]
    fn a_plain_line_goes_through_trimmed() {
        assert_eq!(sanitize("  build this flow  ").unwrap(), "build this flow");
        assert_eq!(
            sanitize("acentuação e emoji 🎨").unwrap(),
            "acentuação e emoji 🎨"
        );
    }

    #[test]
    fn an_escape_is_refused_rather_than_stripped() {
        // The send writes bytes into a live terminal. An instruction the
        // person cannot see being altered is worse than one that does
        // not go.
        assert!(sanitize("clear\x1b[2J").is_err());
        assert!(sanitize("a\x07b").is_err());
        assert!(sanitize("two\ttabs").is_err(), "not even a tab");
        assert!(
            sanitize("carriage\rreturn").is_err(),
            "a break is a newline"
        );
        // What sits at either end is whitespace the trim takes, exactly
        // as a leading space is: the line is measured after it, so what
        // is refused is what would actually have been sent.
        assert_eq!(sanitize("\t tab \n").unwrap(), "tab");
    }

    #[test]
    fn an_instruction_goes_on_to_another_line_with_a_newline() {
        // It is pasted, bracketed, into the agent's box: a newline in it
        // is a line of the same message, never an early submit.
        assert_eq!(
            sanitize("draw the flow\n\nthen wire it").unwrap(),
            "draw the flow\n\nthen wire it"
        );
    }

    #[test]
    fn an_instruction_is_room_for_more_than_the_box_shows_at_once() {
        // The box scrolls past twenty lines; the cap is well past that.
        let long = "a line of an instruction, eighty characters wide or so, give or take\n";
        let long = long.repeat(40);
        assert!(sanitize(&long).is_ok());
    }

    #[test]
    fn an_empty_line_is_refused() {
        assert!(sanitize("   ").is_err());
    }

    #[test]
    fn a_line_past_the_cap_is_refused() {
        assert!(sanitize(&"x".repeat(PROMPT_MAX + 1)).is_err());
    }

    #[test]
    fn the_prompt_puts_the_line_first_and_the_paths_under_it() {
        let files = vec![
            "docs/boards/auth/board.png".to_owned(),
            "docs/boards/auth/board.json".to_owned(),
        ];
        let p = prompt("implement this flow", &files);
        assert!(p.starts_with("implement this flow\n"));
        assert!(p.contains("\n  docs/boards/auth/board.png\n"));
        assert!(p.contains("Diagram exported from the board:"));
    }

    #[test]
    fn the_prompt_names_the_files_relative_to_where_the_agent_is() {
        // The agent is running in the directory the files were written
        // into, so an absolute path would say where the person's home
        // is for no reason.
        let files = vec!["docs/boards/a/board.png".to_owned()];
        assert!(!prompt("go", &files).contains("/home/"));
    }

    #[test]
    fn a_relative_path_is_what_the_files_reduce_to() {
        let files = [std::path::PathBuf::from(
            "/home/e/Work/a/docs/boards/x/board.png",
        )];
        let rel = relative(&files, std::path::Path::new("/home/e/Work/a"));
        assert_eq!(rel, ["docs/boards/x/board.png"]);
    }
}
