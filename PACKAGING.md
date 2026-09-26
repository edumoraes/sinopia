# Packaging

Sinopia reaches people through the AUR, as three packages built from one
GitHub release, and its bar widget through a mirror that `omarchy plugin
add` can clone. A release is a tag; everything after the tag is CI.

## What a tag does

Pushing `vX.Y.Z` runs [release.yml](.github/workflows/release.yml):

1. **Checks** that the tag is `Cargo.toml`'s version, that
   `plugin/manifest.json` carries the same one — the plugin is released with
   the engine — and that `packaging/release-notes/vX.Y.Z.md` is written:
   before anything is built, since a release that would be refused should
   not cost a build first.
2. **Builds** the binary on Ubuntu 24.04. The glibc it links against, 2.39,
   is the oldest it runs on, and Arch is always newer.
   [archive.sh](packaging/archive.sh) packs it into
   `sinopia-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz` with every file the
   packages install, at the paths they have in the source tree, then unpacks
   the archive and installs from inside it — so an archive missing a file
   fails there, and not in somebody's `makepkg`.
3. **Publishes the GitHub release**: that archive, the source as
   `sinopia-vX.Y.Z-source.tar.gz` (`git archive` of the tag — a recipe pins a
   checksum, so the bytes behind it have to be the release's own), and
   `checksums.txt` over both, with the written notes as its body. A tag with
   a suffix (`v0.2.0-rc1`) is a pre-release, and stops here.
4. **Packages** through [packaging.yml](.github/workflows/packaging.yml):
   [native-packages](https://github.com/crmne/native-packages) downloads the
   two archives, holds them to `checksums.txt` and fills the three recipes
   in. `sinopia-bin` and `sinopia` are then built, installed and removed
   in a clean Arch container, and only after both pass are the recipes
   attached to the release and — once `PUBLISH_AUR` is on — pushed to the
   AUR. An AUR package reaches people on their next update, so a broken
   one is found in CI or by them.
5. **Mirrors the plugin** to sinopia-plugin, once `PUBLISH_PLUGIN` is on —
   see [The plugin](#the-plugin).

## The three packages

| Package | Installs | Built from |
| --- | --- | --- |
| `sinopia-bin` | the release's own binary | the release archive |
| `sinopia` | the release, compiled on the machine | the source archive |
| `sinopia-git` | the latest commit, compiled on the machine | the repository |

Every recipe ends in [install.sh](packaging/install.sh): the prebuilt one
runs it from inside the release archive, the other two from the source
tree, and both keep the files at the same paths — so the three install the
same things by construction:

```
/usr/bin/sinopia
/usr/share/applications/sinopia.desktop
/usr/share/icons/hicolor/scalable/apps/sinopia.svg      assets/logo/app-icon.svg
/usr/share/licenses/<package>/                          LICENSE, THIRD-PARTY.md, the font's licence
/usr/share/doc/<package>/README.md
/usr/share/sinopia/plugin/                              the bar widget
/usr/share/sinopia/omarchy/sinopia                      the theme-set / font-set hook
/usr/share/sinopia/skills/sinopia/                      the agent skill and its installer
```

What is under `/usr/share/sinopia` is opt-in. A package never writes into a
home directory, so each person switches those on for themselves, and the
install message ([sinopia.install](packaging/arch/sinopia.install)) says
how.

The binary links glibc and libgcc and nothing else. Wayland, xkbcommon and
Vulkan are loaded at startup, which no inspection of the ELF can see, so the
recipes name them by hand; `namcap` answering that they "may not be needed"
is expected. The window's app id is `sinopia`, the desktop entry's name,
which is what the compositor and the launcher match it by.

## Cutting a release

1. Set the version in `Cargo.toml` and in `plugin/manifest.json`, which
   follows it (§10.3), and write `packaging/release-notes/vX.Y.Z.md`. Commit.
2. `git tag vX.Y.Z && git push origin vX.Y.Z`.

A release whose packaging failed can be packaged again without a new tag:
run **Packaging** by hand from the Actions tab with the version, and
`publish` ticked. Tags and published archives are never rewritten; a fix
to what is inside an archive is a new version.

## The AUR, the first time

native-packages updates an AUR package; it cannot create one — an empty AUR
repository has no `master` for it to stage onto. So the first version of
each of the three is pushed by hand, once the first release is published,
from an AUR account with an SSH key registered:

```sh
gh release download v0.1.0 --pattern 'sinopia-0.1.0-packaging.tar.xz'
mkdir recipes && tar -C recipes -xJf sinopia-0.1.0-packaging.tar.xz
for p in sinopia sinopia-bin sinopia-git; do
  git clone "ssh://aur@aur.archlinux.org/$p.git" "aur-$p"
  cp -a "recipes/arch/$p/." "aur-$p/"
  git -C "aur-$p" add -A
  git -C "aur-$p" commit -m "$p 0.1.0"
  git -C "aur-$p" push origin HEAD:master
done
```

Then, for every release after it to reach the AUR on its own:

- secret `AUR_SSH_KEY`: a private key registered on the AUR account, kept
  for this and nothing else;
- secret `AUR_KNOWN_HOSTS`: `ssh-keyscan aur.archlinux.org`, checked against
  the fingerprints on the AUR's own home page;
- repository variable `PUBLISH_AUR` set to `true`.

Until then a release still gets its archives, its checksums and its recipes
attached; only the push waits.

## The plugin

The bar widget in `plugin/` is developed here, beside the engine it drives,
and reaches people two ways: inside the packages above, at
`/usr/share/sinopia/plugin`, and from
[sinopia-plugin](https://github.com/edumoraes/sinopia-plugin) — a mirror
holding `plugin/` and nothing else, which is what `omarchy plugin add` needs:
it clones a repository whole and wants the manifest at the root. That rules
this repository out twice over, since Omarchy's validator also refuses any
symlink inside a plugin folder, and `CLAUDE.md` is one.

[plugin.yml](.github/workflows/plugin.yml) writes the mirror at every
release, once the engine is packaged, so the mirror is never further along
than the AUR. It splits `plugin/`'s own history out of the tag (`git subtree
split`: the same commits always split into the same ones, so every release
fast-forwards the mirror), runs Omarchy's own validator — pinned — on exactly
the tree it is about to push, and pushes `main` and the tag atomically. A
mirror that anything else has written to refuses the push, and the job fails
rather than overwrite what is there: changes go here, never to the mirror. On
a pull request that touches `plugin/`, the same validator runs, and
`plugin/LICENSE` is held to the root's.

### The mirror, the first time

The job is off until the mirror exists and can be written to:

```sh
gh repo create edumoraes/sinopia-plugin --public \
  --description "Omarchy bar widget for the Sinopia whiteboard"
ssh-keygen -t ed25519 -N '' -C sinopia-plugin-mirror -f plugin-mirror
gh repo deploy-key add plugin-mirror.pub --repo edumoraes/sinopia-plugin \
  --allow-write --title "sinopia releases"
gh secret set PLUGIN_DEPLOY_KEY --repo edumoraes/omawhite < plugin-mirror
rm plugin-mirror plugin-mirror.pub
gh variable set PUBLISH_PLUGIN --repo edumoraes/omawhite --body true
```

The deploy key writes to the mirror and to nothing else. Set up before a
release is tagged, the release fills the mirror in itself; after, run
**Plugin** by hand from the Actions tab with that release's version.

### The marketplace

Once the mirror holds a release and `sinopia-bin` is on the AUR, the widget
can be listed at [plugins.omarchy.org](https://plugins.omarchy.org): an issue
on `omacom/omarchy-plugin-marketplace`, written the way its `SUBMISSION.md`
says — category `Productivity`, tags `ai`, `bar` and `launcher`. A maintainer
approves the listing, and the automated baseline should ask for that review
rather than pass on its own: the README tells people to `yay -S` the engine,
which it reads as the `package-manager` capability. After the listing, each
release shows as "Update unverified" until the verification form is filed
with the mirror's new HEAD. `omarchy plugin add` and `update` clone HEAD
either way, so nobody waits on it for the release itself.

The plugin id becomes permanent with the listing — the marketplace never
frees one — so it is settled before submitting. Today it is `edu.sinopia`,
named in the manifest, in `BarWidget.qml` (its `moduleName`, and the
`ipcTarget` a keybinding calls), in the plugin's README and in the packages'
install message.

## Trying it locally

The whole chain runs on an Arch machine. native-packages is a gem (Arch's
Ruby installs it per user), and a recipe's `.SRCINFO` comes from `makepkg`:

```sh
gem install native-packages --version 0.7.0
cargo build --release --locked
sh packaging/archive.sh v0.1.0 target/release/sinopia dist
git archive --format=tar.gz --prefix=sinopia-0.1.0/ -o dist/sinopia-v0.1.0-source.tar.gz HEAD
native-packages build --version 0.1.0 --output target/recipes
```

`target/recipes/recipes/arch/*/` then holds the three recipes, filled in
from the local archives. To build one, copy the archive it names beside it
— `makepkg` takes a file already there instead of downloading it — and
check the result:

```sh
cd target/recipes/recipes/arch/sinopia-bin
cp ../../../../../dist/sinopia-v0.1.0-x86_64-unknown-linux-gnu.tar.gz .
makepkg -f && namcap ./*.pkg.tar.zst
```

The source package compiles the whole tree twice, once for the build and
once for its tests, which is more than a `tmpfs` `/tmp` of a few gigabytes
holds: build it on disk.

To stage what CI would push, without pushing: `native-packages stage aur
target/recipes/recipes`, then `native-packages diff aur`. The clones land in
`.cache/`, which git ignores.

## Upgrading native-packages

Change `tool.version` in [native-packages.yaml](native-packages.yaml). The
workflows install the version written there.
