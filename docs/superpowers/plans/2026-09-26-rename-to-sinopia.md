# Rename: Omawhite becomes Sinopia

The app takes the name `docs/sinopia/README.md` tells the story of. The
working name comes out of the binary, the paths, the packages, the plugin,
the skill and the docs that describe what exists; it stays only where it
names something that already exists under it.

**Done means:** CI green on the PR (CI, Packaging, Plugin), and the whole
domain using the new name wherever a name is needed.

Branch `rename-sinopia`, stacked on `readme-sinopia` (#11), worked in the
worktree `~/Work/board-rename` so the bar plugin — a symlink into the main
checkout — and the running instance stay untouched.

## Names

| what | was | becomes |
|---|---|---|
| crate, binary, command | `omawhite` | `sinopia` |
| display name, window title | Omawhite | Sinopia |
| data directory | `~/.local/share/omawhite` | `~/.local/share/sinopia` |
| socket | `$XDG_RUNTIME_DIR/omawhite.sock` | `$XDG_RUNTIME_DIR/sinopia.sock` |
| project file extension | `.omawhite` | `.sinopia` |
| clipboard MIME | `application/x-omawhite-layers+json` | `application/x-sinopia-layers+json` |
| Wayland app id, desktop entry, icon | `omawhite` | `sinopia` |
| AUR packages | `omawhite`, `-bin`, `-git` | `sinopia`, `-bin`, `-git` |
| release archives | `omawhite-v…` | `sinopia-v…` |
| plugin id | `edu.omawhite` | `edu.sinopia` |
| plugin mirror | `edumoraes/omawhite-plugin` | `edumoraes/sinopia-plugin` |
| agent skill | `omawhite` | `sinopia` |
| theme hook | `contrib/omarchy/omawhite` | `contrib/omarchy/sinopia` |
| GitHub repository | `edumoraes/omawhite` | `edumoraes/sinopia` (renamed on GitHub, which redirects the old address) |

## What keeps the old name, and why

- **What already exists under it** is read, never written: a data
  directory from before is moved over once, on the first start (refused
  while an instance under the old name is still running, since its unsaved
  drafts live there), and a `.omawhite` file still shows in Open.
- **History**: `docs/superpowers/` records what was planned and decided at
  the time, under the name it had.

## Tasks

- [x] 1. Code: crate and binary, CLI, socket, data directory, extension,
  MIME, app id and window title, dialog filter and threads, tmux buffer,
  export inventory's marker, gfx labels, messages, tests.
- [x] 2. Migration: move the old data directory once (not while the old
  instance runs); the old extension in Open. Tested.
- [x] 3. Packaging: `install.sh`, `archive.sh`, desktop entry, the three
  recipes and the install message, `native-packages.yaml`, release notes.
- [x] 4. Workflows: `ci.yml`, `release.yml`, `packaging.yml`, `plugin.yml`.
- [x] 5. Plugin: manifest, widget, README.
- [x] 6. Skill: directory, `SKILL.md`, installer.
- [x] 7. Theme hook.
- [x] 8. Logo: the wordmark spells Sinopia (`tools/logo.py`: S, n, o, p and
  a descender), regenerated.
- [x] 9. Docs: README, DEVELOPMENT, AGENTS, ARCHITECTURE, PACKAGING,
  THIRD-PARTY, the Sinopia page's footnote.
- [x] 10. Local checks: `cargo build`, `cargo clippy --all-targets` with
  `-D warnings`, `cargo test` (1063), `archive.sh`, `desktop-file-validate`,
  Omarchy's plugin validator. `native-packages validate` is left to CI's
  Packaging job: the gem is not installed on this machine.
- [x] 11. Live check: smoke test, then a live instance answering
  `sinopia agent frames` and `sinopia layer list`; the migration against a
  scratch data directory. The instance answered `agent add/frames/read` and
  `layer list/add` on `sinopia.sock`, under app id `sinopia`; the move was
  refused while the real `omawhite` answered, and made without it. The
  clipboard's `Backend error` on the way out predates the rename: the old
  binary prints it too.
- [x] 12. PR stacked on #11 (#12); CI, Packaging and Plugin green — the
  Recipes job ran `native-packages validate` and `desktop-file-validate`.

## After merging, on this machine

Not done by the PR — the environment is the person's:

- close the running `omawhite`, install `sinopia` (`~/.local/bin`), start
  it once to bring `~/.local/share/omawhite` over;
- the bar plugin: re-point `~/.config/omarchy/plugins/edu.omawhite` as
  `edu.sinopia` and enable that id;
- the skill: remove `skills/omawhite` from Claude Code, Codex and
  OpenCode, run `skills/sinopia/install.sh`;
- a theme hook installed under the old name, if any.
