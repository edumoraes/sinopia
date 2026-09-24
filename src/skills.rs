//! The skills of the harness a prompt is going to: where each harness
//! keeps them, which of them a person may call, and how a call is written
//! so that the harness takes it as one. Listing the folders and reading
//! the files is shell; everything that decides what they meant is pure.
//!
//! Five harnesses, five answers — read off their own sources, and written
//! down in [`harness`] so that nobody derives them twice.

use std::path::{Path, PathBuf};

use crate::field::Field;
use crate::store;

/// The most skills one harness is shown with, the most entries read from
/// one folder, and the most of one `SKILL.md` that is read: its
/// frontmatter is at the top. A folder of skills lives in somebody's
/// repository, and listing it happens on the window's own thread.
const MAX_SKILLS: usize = 512;
const MAX_ENTRIES: usize = 512;
const MAX_HEAD: u64 = 64 * 1024;

/// How a harness is told, in the prompt itself, to use a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    /// `/name`, first thing in the message: Claude Code, OpenCode and
    /// Gemini CLI.
    Slash,
    /// `$name`, anywhere in it: Codex.
    Dollar,
    /// No typed call at all: Crush offers its skills to its model and to
    /// its palette, never to a line of text, so the skill is named in
    /// words and the model takes it up.
    Words,
}

/// Which folders a project's skills are looked for in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Walk {
    /// The working folder only.
    Here,
    /// The working folder and the repository's root.
    HereAndRoot,
    /// Every folder from the working one up to the repository's root.
    UpToRoot,
}

/// Where one harness keeps its skills and how it is asked for one.
#[derive(Debug, Clone, Copy)]
pub struct Harness {
    pub call: Call,
    /// Folders under the person's home, in the order they are read.
    home: &'static [&'static str],
    /// Folders under the XDG config directory.
    config: &'static [&'static str],
    /// Folders anywhere on the machine.
    system: &'static [&'static str],
    /// Folders under each project folder the walk names.
    project: &'static [&'static str],
    walk: Walk,
    /// A skill is called by its folder's name, whatever its frontmatter
    /// says — Claude Code's rule, where `name` is only a label.
    by_folder: bool,
    /// Claude Code's plugins bring skills of their own.
    plugins: bool,
}

/// The harness an agent of `kind` is, when this build knows it.
pub fn harness(kind: &str) -> Option<Harness> {
    let h = match kind {
        "claude" => Harness {
            call: Call::Slash,
            home: &[".claude/skills"],
            config: &[],
            system: &[],
            project: &[".claude/skills"],
            walk: Walk::UpToRoot,
            by_folder: true,
            plugins: true,
        },
        "codex" => Harness {
            call: Call::Dollar,
            home: &[".agents/skills", ".codex/skills", ".codex/skills/.system"],
            config: &[],
            system: &["/etc/codex/skills"],
            project: &[".agents/skills", ".codex/skills"],
            walk: Walk::UpToRoot,
            by_folder: false,
            plugins: false,
        },
        "opencode" => Harness {
            call: Call::Slash,
            home: &[
                ".claude/skills",
                ".agents/skills",
                ".opencode/skill",
                ".opencode/skills",
            ],
            config: &["opencode/skill", "opencode/skills"],
            system: &[],
            project: &[
                ".claude/skills",
                ".agents/skills",
                ".opencode/skill",
                ".opencode/skills",
            ],
            walk: Walk::UpToRoot,
            by_folder: false,
            plugins: false,
        },
        "crush" => Harness {
            call: Call::Words,
            home: &[".agents/skills", ".claude/skills"],
            config: &["crush/skills", "agents/skills"],
            system: &[],
            project: &[
                ".agents/skills",
                ".crush/skills",
                ".claude/skills",
                ".cursor/skills",
            ],
            walk: Walk::HereAndRoot,
            by_folder: false,
            plugins: false,
        },
        "gemini" => Harness {
            call: Call::Slash,
            home: &[".gemini/skills", ".agents/skills"],
            config: &[],
            system: &[],
            project: &[".gemini/skills", ".agents/skills"],
            walk: Walk::Here,
            by_folder: false,
            plugins: false,
        },
        _ => return None,
    };
    Some(h)
}

/// One skill a person may call: the name it is called by, plugin and all,
/// and what it says it is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
}

impl Harness {
    /// Every folder whose subfolders hold a `SKILL.md`, nearest first:
    /// the project's — as far up it as this harness looks, which is
    /// never past the repository's root, and is only the working folder
    /// where there is no repository — then the person's own, then the
    /// machine's. A name found twice is the nearer one's.
    pub fn roots(
        &self,
        home: &Path,
        config: &Path,
        cwd: &Path,
        repo: Option<&Path>,
    ) -> Vec<PathBuf> {
        let mut folders: Vec<&Path> = match (self.walk, repo) {
            (Walk::Here, _) | (_, None) => vec![cwd],
            (Walk::HereAndRoot, Some(root)) => vec![cwd, root],
            (Walk::UpToRoot, Some(root)) => cwd
                .ancestors()
                .take_while(|a| a.starts_with(root))
                .collect(),
        };
        folders.dedup();
        if folders.is_empty() {
            folders.push(cwd);
        }
        let mut out = Vec::new();
        for folder in folders {
            out.extend(self.project.iter().map(|d| folder.join(d)));
        }
        out.extend(self.home.iter().map(|d| home.join(d)));
        out.extend(self.config.iter().map(|d| config.join(d)));
        out.extend(self.system.iter().map(PathBuf::from));
        out
    }

    /// The skill a `SKILL.md` in `folder` describes, as this harness
    /// calls it — or none, when it may not be called from a prompt or its
    /// name could not be typed after the call's own mark.
    pub fn skill(&self, front: &Front, folder: &str, plugin: Option<&str>) -> Option<Skill> {
        let own = if self.by_folder && plugin.is_none() {
            folder
        } else {
            front.name.as_deref().unwrap_or(folder)
        };
        let name = match plugin {
            Some(p) => format!("{p}:{own}"),
            None => own.to_owned(),
        };
        // Crush's skills are called through its model, so what decides is
        // whether the model may call it; everywhere else, the person.
        let offered = match self.call {
            Call::Words => front.model_invocable,
            _ => front.user_invocable,
        };
        (offered && callable(&name)).then(|| Skill {
            name,
            description: front.description.clone(),
        })
    }
}

/// Whether `name` can go into a prompt as a call: a letter or digit, then
/// letters, digits and `-_.:` — a plugin's colon included — and short.
/// A skill's name comes out of a file in somebody's repository, and it is
/// about to be written into a live terminal: anything else is not called.
pub fn callable(name: &str) -> bool {
    name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// What a `SKILL.md` says about itself between its two `---` lines —
/// only the four keys a call needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Front {
    pub name: Option<String>,
    /// On one line, whatever it was written as: it is shown on one row.
    pub description: String,
    /// `user-invocable`, which is true unless the file says otherwise.
    pub user_invocable: bool,
    /// Not `disable-model-invocation: true`.
    pub model_invocable: bool,
}

/// A value without the quotes around it, when it has a pair.
fn unquote(v: &str) -> &str {
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|v| v.strip_suffix(q)) {
            return inner;
        }
    }
    v
}

/// Whether a frontmatter line belongs to the key above it.
fn indented(line: &str) -> bool {
    line.starts_with([' ', '\t']) || line.trim().is_empty()
}

/// A `SKILL.md`'s frontmatter, or none where it has none. This is the
/// sliver of YAML the files are written in — top-level `key: value`, a
/// quoted value, a plain one running on over indented lines, and the
/// `|` and `>` blocks — and everything a key's value holds is read as one
/// line of words, since that is how it is shown. Anything nested under a
/// key is passed over.
pub fn frontmatter(text: &str) -> Option<Front> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    let mut body = Vec::new();
    let mut closed = false;
    for line in lines {
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        body.push(line);
    }
    if !closed {
        return None;
    }
    let mut front = Front {
        name: None,
        description: String::new(),
        user_invocable: true,
        model_invocable: true,
    };
    let mut i = 0;
    while i < body.len() {
        let line = body[i];
        i += 1;
        if indented(line) || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let block = matches!(value, "|" | ">" | "|-" | ">-" | "|+" | ">+");
        let mut words = vec![if block { "" } else { unquote(value) }];
        // A block's lines, or a plain value's continuation: an empty
        // value's indented lines are a nested map, and are not its.
        if block || !value.is_empty() {
            while i < body.len() && indented(body[i]) {
                words.push(body[i].trim());
                i += 1;
            }
        }
        let value: String = words
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        match key.trim() {
            "name" => front.name = Some(value).filter(|v| !v.is_empty()),
            "description" => front.description = value,
            "user-invocable" => front.user_invocable = value != "false",
            "disable-model-invocation" => front.model_invocable = value != "true",
            _ => {}
        }
    }
    Some(front)
}

/// The mark a call starts with, which is also what opens the list in the
/// dialog. Crush has no call of its own to type, so it borrows the slash
/// that opens its palette.
pub fn mark(call: Call) -> char {
    match call {
        Call::Dollar => '$',
        Call::Slash | Call::Words => '/',
    }
}

/// How the call to `name` is written for a harness that takes `call`.
pub fn call_text(call: Call, name: &str) -> String {
    match call {
        Call::Slash => format!("/{name}"),
        Call::Dollar => format!("${name}"),
        Call::Words => format!("Use the {name} skill."),
    }
}

/// A call being typed: the characters `start..end` it spans in the text,
/// counted as a field's caret is, and what has been typed of the name so
/// far — the part between its mark and the caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub query: String,
}

/// The call the caret is in the middle of typing, if it is: a word that
/// starts with the harness's mark — first in the message for a slash,
/// which is the only place one is read; at the start of any word for a
/// dollar — with the caret past the mark and nothing between the two a
/// name could not hold. A space typed after a name ends it, and with it
/// the list.
pub fn token(text: &str, caret: usize, call: Call) -> Option<Token> {
    let chars: Vec<char> = text.chars().collect();
    let caret = caret.min(chars.len());
    let mut start = caret;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    if caret <= start || chars[start] != mark(call) {
        return None;
    }
    if call != Call::Dollar && start != 0 {
        return None;
    }
    let query: String = chars[start + 1..caret].iter().collect();
    if !query
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        return None;
    }
    let mut end = caret;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    Some(Token { start, end, query })
}

/// Which of `skills` answer to `query`, best first: those whose name
/// starts with it — or whose own name does, after a plugin's colon — then
/// those that merely hold it, each in the order they were given. Case is
/// not what anybody means.
pub fn matching(skills: &[Skill], query: &str) -> Vec<usize> {
    let query = query.to_ascii_lowercase();
    let (mut starts, mut holds) = (Vec::new(), Vec::new());
    for (i, skill) in skills.iter().enumerate() {
        let name = skill.name.to_ascii_lowercase();
        let own = name.rsplit(':').next().unwrap_or(&name);
        if name.starts_with(&query) || own.starts_with(&query) {
            starts.push(i);
        } else if name.contains(&query) {
            holds.push(i);
        }
    }
    starts.extend(holds);
    starts
}

/// Writes the call to `name` over what had been typed of it, and a space
/// after it to go on typing from — or past the one already there.
pub fn accept(line: &mut Field, token: &Token, call: Call, name: &str) {
    let spaced = line
        .value()
        .chars()
        .nth(token.end)
        .is_some_and(char::is_whitespace);
    line.go(token.start, false);
    line.go(token.end, true);
    line.insert_str(&call_text(call, name));
    if spaced {
        line.right(false);
    } else {
        line.insert(' ');
    }
}

/// The skills in `roots` — every subfolder holding a `SKILL.md` — then
/// the ones each plugin brings under its own `skills/`, in that order of
/// precedence: a name met twice is the first one's. Listed by name. A
/// folder that is not there, or a file that will not read or parse, is
/// passed over: a skill nobody can see is not worth refusing a list for.
pub fn gather(h: &Harness, roots: &[PathBuf], plugins: &[(String, PathBuf)]) -> Vec<Skill> {
    let mut out: Vec<Skill> = Vec::new();
    let mut read = |folder: &Path, plugin: Option<&str>| {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten().take(MAX_ENTRIES) {
            if out.len() >= MAX_SKILLS {
                return;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(head) = store::read_head(&entry.path().join("SKILL.md"), MAX_HEAD) else {
                continue;
            };
            let Some(front) = frontmatter(&String::from_utf8_lossy(&head)) else {
                continue;
            };
            if let Some(skill) = h.skill(&front, name, plugin)
                && !out.iter().any(|s| s.name == skill.name)
            {
                out.push(skill);
            }
        }
    };
    for root in roots {
        read(root, None);
    }
    for (name, install) in plugins {
        read(&install.join("skills"), Some(name));
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The repository `cwd` is in: the nearest folder at or above it that
/// holds a `.git` — a directory, or the file a worktree has instead.
pub fn repo_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|a| a.join(".git").exists())
        .map(Path::to_path_buf)
}

/// What an agent of `kind` working in `cwd` can be asked to use: its
/// harness's skills, read from where that harness reads them. Nothing,
/// for a harness this build does not know.
pub fn list(kind: &str, cwd: &str) -> Vec<Skill> {
    let Some(h) = harness(kind) else {
        return Vec::new();
    };
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let cwd = Path::new(cwd);
    let repo = repo_root(cwd);
    let roots = h.roots(&home, &config, cwd, repo.as_deref());
    let plugins = if h.plugins {
        claude_plugins(&home)
    } else {
        Vec::new()
    };
    gather(&h, &roots, &plugins)
}

/// Claude Code's own record of its plugins, and its settings, read.
fn claude_plugins(home: &Path) -> Vec<(String, PathBuf)> {
    let read = |path: PathBuf| {
        store::read_capped(&path, 4 * 1024 * 1024)
            .ok()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    };
    let base = home.join(".claude");
    match (
        read(base.join("plugins/installed_plugins.json")),
        read(base.join("settings.json")),
    ) {
        (Some(installed), Some(settings)) => plugins(&installed, &settings),
        _ => Vec::new(),
    }
}

/// Claude Code's plugins that bring skills to every project: installed
/// for the person rather than for one project, and switched on. Each
/// comes back as the name its skills are called through — `firecrawl`
/// for `firecrawl@official` — and where it is installed. What will not
/// parse is no plugins, not an error: a skill list is a convenience.
pub fn plugins(installed: &str, settings: &str) -> Vec<(String, PathBuf)> {
    let parse = |s| serde_json::from_str::<serde_json::Value>(s).ok();
    let (Some(installed), Some(settings)) = (parse(installed), parse(settings)) else {
        return Vec::new();
    };
    let Some(all) = installed["plugins"].as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, installs) in all {
        if settings["enabledPlugins"][key].as_bool() != Some(true) {
            continue;
        }
        let Some(name) = key.split('@').next().filter(|n| callable(n)) else {
            continue;
        };
        for install in installs.as_array().into_iter().flatten() {
            if install["scope"].as_str() == Some("user")
                && let Some(path) = install["installPath"].as_str()
            {
                out.push((name.to_owned(), PathBuf::from(path)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn a_folded_description_is_read_as_one_line() {
        let text = "---\nname: omarchy\ndescription: >\n  REQUIRED for end-user customization.\n  Use when editing ~/.config/hypr/.\n---\n\n# Omarchy\n";
        let front = frontmatter(text).unwrap();
        assert_eq!(front.name.as_deref(), Some("omarchy"));
        assert_eq!(
            front.description,
            "REQUIRED for end-user customization. Use when editing ~/.config/hypr/."
        );
    }

    #[test]
    fn a_literal_block_and_a_quoted_value_read_as_what_they_say() {
        let text = "---\nname: \"basecamp\"\ndescription: |\n  Interact with Basecamp.\n  Use for ANY Basecamp question.\ntriggers:\n  - basecamp\n---\nbody";
        let front = frontmatter(text).unwrap();
        assert_eq!(front.name.as_deref(), Some("basecamp"));
        assert_eq!(
            front.description,
            "Interact with Basecamp. Use for ANY Basecamp question."
        );
    }

    #[test]
    fn a_skill_may_say_a_person_cannot_call_it_and_a_model_cannot_either() {
        let text = "---\nname: x\ndescription: y\nuser-invocable: false\ndisable-model-invocation: true\n---\n";
        let front = frontmatter(text).unwrap();
        assert!(!front.user_invocable);
        assert!(!front.model_invocable);
        let plain = frontmatter("---\nname: x\n---\n").unwrap();
        assert!(
            plain.user_invocable && plain.model_invocable,
            "both, unless said"
        );
    }

    #[test]
    fn a_file_with_no_frontmatter_is_no_skill() {
        assert!(frontmatter("# just a readme\n").is_none());
        assert!(frontmatter("---\nname: open\n").is_none(), "never closed");
        assert!(frontmatter("\u{feff}---\r\nname: bom\r\n---\r\n").is_some());
    }

    #[test]
    fn a_name_is_what_can_be_typed_after_a_slash_and_nothing_else() {
        for ok in [
            "omarchy",
            "code-review",
            "firecrawl:skill-gen",
            "a_b.c",
            "x9",
        ] {
            assert!(callable(ok), "{ok:?}");
        }
        for bad in ["", "two words", "-lead", "a\nb", "semi;colon", "$(x)", "é"] {
            assert!(!callable(bad), "{bad:?}");
        }
        assert!(!callable(&"a".repeat(65)), "a name is short");
    }

    #[test]
    fn claude_code_reads_its_own_folder_up_the_project_and_its_home() {
        let h = harness("claude").unwrap();
        let roots = h.roots(
            &p("/h"),
            &p("/h/.config"),
            &p("/w/app/web"),
            Some(&p("/w/app")),
        );
        assert_eq!(
            roots,
            [
                p("/w/app/web/.claude/skills"),
                p("/w/app/.claude/skills"),
                p("/h/.claude/skills"),
            ]
        );
    }

    #[test]
    fn codex_reads_the_shared_folder_and_its_own_from_the_project_down() {
        let h = harness("codex").unwrap();
        let roots = h.roots(
            &p("/h"),
            &p("/h/.config"),
            &p("/w/app/web"),
            Some(&p("/w/app")),
        );
        assert_eq!(
            roots,
            [
                p("/w/app/web/.agents/skills"),
                p("/w/app/web/.codex/skills"),
                p("/w/app/.agents/skills"),
                p("/w/app/.codex/skills"),
                p("/h/.agents/skills"),
                p("/h/.codex/skills"),
                p("/h/.codex/skills/.system"),
                p("/etc/codex/skills"),
            ]
        );
    }

    #[test]
    fn gemini_reads_the_working_folder_only_and_crush_the_repository_too() {
        let gemini = harness("gemini").unwrap();
        let roots = gemini.roots(
            &p("/h"),
            &p("/h/.config"),
            &p("/w/app/web"),
            Some(&p("/w/app")),
        );
        assert!(roots.contains(&p("/w/app/web/.gemini/skills")));
        assert!(
            !roots.iter().any(|r| r.starts_with("/w/app/.gemini")),
            "no walk up"
        );
        let crush = harness("crush").unwrap();
        let roots = crush.roots(&p("/h"), &p("/x"), &p("/w/app/web"), Some(&p("/w/app")));
        assert!(roots.contains(&p("/w/app/web/.crush/skills")));
        assert!(roots.contains(&p("/w/app/.crush/skills")));
        assert!(
            roots.contains(&p("/x/crush/skills")),
            "under XDG_CONFIG_HOME"
        );
    }

    #[test]
    fn with_no_repository_only_the_working_folder_is_the_project() {
        let h = harness("opencode").unwrap();
        let roots = h.roots(&p("/h"), &p("/h/.config"), &p("/w/loose"), None);
        let project: Vec<&PathBuf> = roots.iter().filter(|r| r.starts_with("/w")).collect();
        assert!(!project.is_empty());
        assert!(
            project.iter().all(|r| r.starts_with("/w/loose")),
            "{project:?}"
        );
    }

    #[test]
    fn an_agent_this_build_does_not_know_has_no_skills_to_offer() {
        assert!(harness("pi").is_none());
    }

    #[test]
    fn claude_code_names_a_skill_by_its_folder_and_the_others_by_its_frontmatter() {
        let front = frontmatter("---\nname: shown\ndescription: d\n---\n").unwrap();
        let claude = harness("claude").unwrap();
        let codex = harness("codex").unwrap();
        assert_eq!(claude.skill(&front, "folder", None).unwrap().name, "folder");
        assert_eq!(codex.skill(&front, "folder", None).unwrap().name, "shown");
        let nameless = frontmatter("---\ndescription: d\n---\n").unwrap();
        assert_eq!(
            codex.skill(&nameless, "folder", None).unwrap().name,
            "folder"
        );
    }

    #[test]
    fn a_plugins_skill_is_called_through_the_plugin() {
        let front = frontmatter("---\nname: skill-gen\ndescription: d\n---\n").unwrap();
        let claude = harness("claude").unwrap();
        let s = claude
            .skill(&front, "skill-gen", Some("firecrawl"))
            .unwrap();
        assert_eq!(s.name, "firecrawl:skill-gen");
    }

    #[test]
    fn a_skill_nobody_may_call_is_not_offered() {
        let hidden = frontmatter("---\nname: h\nuser-invocable: false\n---\n").unwrap();
        assert!(
            harness("claude")
                .unwrap()
                .skill(&hidden, "h", None)
                .is_none()
        );
        // Crush calls a skill through its model, so what matters there is
        // whether the model may.
        let crush = harness("crush").unwrap();
        assert!(crush.skill(&hidden, "h", None).is_some());
        let unmodelled =
            frontmatter("---\nname: m\ndisable-model-invocation: true\n---\n").unwrap();
        assert!(crush.skill(&unmodelled, "m", None).is_none());
        let odd = frontmatter("---\nname: two words\n---\n").unwrap();
        assert!(harness("codex").unwrap().skill(&odd, "dir", None).is_none());
    }

    fn tok(start: usize, end: usize, query: &str) -> Option<Token> {
        Some(Token {
            start,
            end,
            query: query.to_owned(),
        })
    }

    #[test]
    fn a_slash_opens_the_list_only_first_in_the_message() {
        assert_eq!(token("/fro", 4, Call::Slash), tok(0, 4, "fro"));
        assert_eq!(token("/", 1, Call::Slash), tok(0, 1, ""));
        assert_eq!(token("/fro rest", 2, Call::Slash), tok(0, 4, "f"));
        assert_eq!(token("do /fro", 7, Call::Slash), None, "not first");
        assert_eq!(
            token("/fro and more", 13, Call::Slash),
            None,
            "the caret left it"
        );
        assert_eq!(
            token("/fro", 0, Call::Slash),
            None,
            "the caret is before it"
        );
        assert_eq!(token("/a;b", 4, Call::Slash), None, "no name holds a ;");
    }

    #[test]
    fn a_dollar_opens_the_list_wherever_a_word_starts() {
        assert_eq!(token("use $sk now", 7, Call::Dollar), tok(4, 7, "sk"));
        assert_eq!(token("a$sk", 4, Call::Dollar), None, "inside a word");
    }

    #[test]
    fn crush_opens_its_list_with_a_slash_as_the_others_do() {
        assert_eq!(token("/re", 3, Call::Words), tok(0, 3, "re"));
    }

    fn named(names: &[&str]) -> Vec<Skill> {
        names
            .iter()
            .map(|n| Skill {
                name: (*n).to_owned(),
                description: String::new(),
            })
            .collect()
    }

    #[test]
    fn a_skill_is_matched_by_how_its_name_starts_first_then_by_what_it_holds() {
        let skills = named(&[
            "code-review",
            "firecrawl:skill-gen",
            "frontend-design",
            "review-pr",
        ]);
        assert_eq!(matching(&skills, "rev"), [3, 0]);
        assert_eq!(matching(&skills, ""), [0, 1, 2, 3]);
        assert_eq!(matching(&skills, "FRO"), [2], "whatever the case");
        assert_eq!(
            matching(&skills, "sk"),
            [1],
            "a plugin's skill starts after its colon"
        );
        assert!(matching(&skills, "zzz").is_empty());
    }

    #[test]
    fn a_call_is_written_the_way_its_harness_reads_one() {
        assert_eq!(call_text(Call::Slash, "omarchy"), "/omarchy");
        assert_eq!(call_text(Call::Dollar, "omarchy"), "$omarchy");
        assert_eq!(call_text(Call::Words, "omarchy"), "Use the omarchy skill.");
    }

    #[test]
    fn taking_a_skill_writes_its_call_over_what_was_typed_of_it() {
        let mut f = crate::field::Field::lines("/fro and the rest");
        f.go(3, false);
        let t = token(f.value(), f.caret(), Call::Slash).unwrap();
        accept(&mut f, &t, Call::Slash, "frontend-design");
        assert_eq!(f.value(), "/frontend-design and the rest");
        assert_eq!(
            f.caret(),
            "/frontend-design ".chars().count(),
            "past the space"
        );
        let mut f = crate::field::Field::lines("look at $re");
        let t = token(f.value(), f.caret(), Call::Dollar).unwrap();
        accept(&mut f, &t, Call::Dollar, "review-pr");
        assert_eq!(
            f.value(),
            "look at $review-pr ",
            "a space to go on typing after"
        );
    }

    fn put(root: &Path, folder: &str, text: &str) {
        std::fs::create_dir_all(root.join(folder)).unwrap();
        std::fs::write(root.join(folder).join("SKILL.md"), text).unwrap();
    }

    fn names(skills: &[Skill]) -> Vec<&str> {
        skills.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn skills_are_gathered_nearest_first_and_listed_by_name() {
        let t = tempfile::tempdir().unwrap();
        let (near, far) = (t.path().join("near"), t.path().join("far"));
        put(
            &near,
            "zeta",
            "---\nname: zeta\ndescription: the near one\n---\n",
        );
        put(
            &far,
            "zeta",
            "---\nname: zeta\ndescription: the far one\n---\n",
        );
        put(&far, "alpha", "---\nname: alpha\ndescription: a\n---\n");
        std::fs::create_dir_all(far.join("no-skill-here")).unwrap();
        let got = gather(
            &harness("codex").unwrap(),
            &[near, far, t.path().join("gone")],
            &[],
        );
        assert_eq!(names(&got), ["alpha", "zeta"]);
        assert_eq!(got[1].description, "the near one");
    }

    #[test]
    fn a_plugins_skills_are_gathered_under_its_name() {
        let t = tempfile::tempdir().unwrap();
        let install = t.path().join("firecrawl/1.0.9");
        put(
            &install.join("skills"),
            "skill-gen",
            "---\nname: skill-gen\ndescription: d\n---\n",
        );
        let claude = harness("claude").unwrap();
        let got = gather(&claude, &[], &[("firecrawl".to_owned(), install)]);
        assert_eq!(names(&got), ["firecrawl:skill-gen"]);
    }

    #[test]
    fn the_repository_is_the_nearest_folder_holding_a_git() {
        let t = tempfile::tempdir().unwrap();
        let repo = t.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("a/b")).unwrap();
        assert_eq!(repo_root(&repo.join("a/b")), Some(repo.clone()));
        // A worktree's `.git` is a file, and a repository all the same.
        let tree = t.path().join("tree");
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::write(tree.join(".git"), "gitdir: /elsewhere\n").unwrap();
        assert_eq!(repo_root(&tree), Some(tree.clone()));
    }

    #[test]
    fn claude_codes_plugins_are_the_enabled_ones_installed_for_the_user() {
        let installed = r#"{"version": 2, "plugins": {
            "firecrawl@official": [{"scope": "user", "installPath": "/c/firecrawl/1.0.9"}],
            "playwright@official": [{"scope": "user", "installPath": "/c/playwright/1"}],
            "local@official": [{"scope": "project", "installPath": "/c/local/1", "projectPath": "/w"}],
            "stray@official": [{"scope": "user", "installPath": "/c/stray/1"}]
        }}"#;
        let settings = r#"{"enabledPlugins": {
            "firecrawl@official": true, "playwright@official": false, "local@official": true
        }}"#;
        assert_eq!(
            plugins(installed, settings),
            [("firecrawl".to_owned(), p("/c/firecrawl/1.0.9"))]
        );
        assert!(plugins("not json", settings).is_empty());
        let _ = Path::new("/");
    }
}
