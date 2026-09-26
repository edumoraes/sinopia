import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// Sinopia on the bar: an icon whose popout holds the two ways into the
// board — a new one, or one of the recent projects.
//
// The plugin speaks intent and nothing else (§3). It does not interpret a
// document: it reads `index.json`, which is the one file it is allowed to
// read (§6), and every gesture leaves through the CLI that mirrors the
// socket (§5). The child is launched detached, so it owns its own window
// (§4.1) and killing it cannot take the shell down with it (§11).
//
// Ui/Panel hands over the whole popout contract: open, close, toggle,
// opened, switchPanel, setting and the IpcHandler.
Panel {
  id: root
  moduleName: "edu.sinopia"
  ipcTarget: "edu.sinopia"

  // How many rows the list shows at once. The index keeps everything, so
  // what falls past this is reached by typing rather than by scrolling
  // forever — and with a filter the rows are drawn from the matches, so
  // the twentieth-oldest project is one word away.
  readonly property int shownMax: 20

  // The bar item follows the Panel's foreground (transparency-aware); the
  // popout's content follows the theme's popup surface.
  readonly property color foreground: Color.popups.text

  // "menu" | "open". The second screen is the recents; Esc there clears
  // the filter, then steps back to the first, and only then closes.
  property string screen: "menu"

  // Every recent project, newest first: a draft, which the store keeps
  // and which is opened by id, or a file the person named, which is
  // opened by path. Both come out of `index.json` (§6) and both are
  // untrusted text — drawn as plain text, and an id checked against what
  // the engine itself accepts before it is ever an argument.
  property var projects: []

  // The paths that are not there right now, as a set. A recent whose
  // file has gone keeps its place — a drive nobody has mounted is not a
  // deletion — and says so instead of pretending.
  property var missing: ({})

  // What has been typed on the list screen. Reaches the whole index, not
  // just the rows on show.
  property string filter: ""

  // The engine, resolved when the popout opens (§10.1). "" while nobody
  // has answered; `searched` separates that from "it is not installed".
  property string enginePath: ""
  property bool searched: false
  readonly property bool haveEngine: enginePath !== ""

  // One cursor for whichever screen is up. `cursorLive` is what keeps a
  // single highlight on screen: the keyboard raises it, and the mouse
  // moves it rather than painting a second one (the CursorSurface
  // contract).
  property int cursor: 0
  property bool cursorLive: false

  // The rows actually drawn: the matches, capped. Bound rather than
  // rebuilt by hand so a change to the index, the filter or what is
  // missing all reach the list by the same road.
  readonly property var shown: {
    var needle = root.filter.toLowerCase()
    var out = []
    for (var i = 0; i < root.projects.length && out.length < root.shownMax; i++) {
      var p = root.projects[i]
      if (needle !== "" && p.name.toLowerCase().indexOf(needle) < 0) {
        continue
      }
      out.push(p)
    }
    return out
  }

  readonly property int rowCount: screen === "menu" ? 2 : shown.length

  implicitWidth: item.implicitWidth
  implicitHeight: item.implicitHeight

  // ------------------------------------------------------------- the index

  // Mirrors `validate_id` in the engine's own store: a row naming an id
  // it would refuse could never be opened, so it is not shown.
  function validId(id) {
    var s = String(id || "")
    return s.length > 0 && s.length <= 64 && /^[A-Za-z0-9_-]+$/.test(s)
  }

  // A file is called by its own name, as the tab calls it: the title
  // travelled inside the JSON, the name is what the person chose.
  function fileName(path) {
    var parts = String(path).split("/")
    var last = parts[parts.length - 1] || ""
    var dot = last.lastIndexOf(".")
    return (dot > 0 ? last.slice(0, dot) : last) || "untitled"
  }

  function absorb(raw) {
    var list = []
    try {
      var d = JSON.parse(String(raw || ""))
      var entries = (d && d.boards) || []
      for (var i = 0; i < entries.length; i++) {
        var e = entries[i]
        if (!e || !root.validId(e.id)) {
          continue
        }
        var path = typeof e.path === "string" && e.path !== "" ? e.path : ""
        var title = String(e.title || "").replace(/\s+/g, " ").trim()
        list.push({
          "id": String(e.id),
          "path": path,
          "name": path !== "" ? root.fileName(path) : (title !== "" ? title : "untitled"),
          "titled": path !== "" || title !== "",
          "updated": Number(e.updated_at) || 0
        })
      }
      list.sort(function (a, b) { return b.updated - a.updated })
    } catch (err) {
      // A half-written index — the engine writes atomically, but a reader
      // can still meet a truncated read — leaves the last good list up.
      return
    }
    root.projects = list
    root.checkPaths()
  }

  function whenText(stamp) {
    var t = Number(stamp)
    if (!isFinite(t) || t <= 0) {
      return ""
    }
    var s = Math.max(0, Math.floor(Date.now() / 1000 - t))
    if (s < 60) return "just now"
    if (s < 3600) {
      var m = Math.floor(s / 60)
      return m + (m === 1 ? " min ago" : " mins ago")
    }
    if (s < 86400) {
      var h = Math.floor(s / 3600)
      return h + (h === 1 ? " hour ago" : " hours ago")
    }
    var d = Math.floor(s / 86400)
    if (d === 1) return "yesterday"
    if (d < 30) return d + " days ago"
    var mo = Math.floor(d / 30)
    return mo + (mo === 1 ? " month ago" : " months ago")
  }

  FileView {
    id: index
    path: Quickshell.env("HOME") + "/.local/share/sinopia/index.json"
    watchChanges: true
    printErrors: false          // no projects yet is a state, not an error
    onLoaded: root.absorb(text())
    onLoadFailed: root.projects = []
    onFileChanged: reload()
  }

  // One pass over the remembered paths, in one process, rather than a
  // watcher per row: the answer only has to be right at the moment the
  // popout opens, and a row that turns out to be wrong is corrected by
  // the engine dropping the entry when it fails to open it.
  function checkPaths() {
    if (probe.running) {
      return
    }
    var paths = []
    for (var i = 0; i < root.projects.length; i++) {
      if (root.projects[i].path !== "") {
        paths.push(root.projects[i].path)
      }
    }
    if (paths.length === 0) {
      root.missing = ({})
      return
    }
    probe.command = ["sh", "-c", 'for p in "$@"; do [ -e "$p" ] || printf "%s\\n" "$p"; done', "sh"].concat(paths)
    probe.running = true
  }

  Process {
    id: probe
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var gone = ({})
        var lines = String(text || "").split("\n")
        for (var i = 0; i < lines.length; i++) {
          if (lines[i] !== "") {
            gone[lines[i]] = true
          }
        }
        root.missing = gone
      }
    }
  }

  // ------------------------------------------------------------ the engine

  // §10.1's search, in order: `sinopia` on the PATH, then ~/.local/bin,
  // then the configurable path, then the dev build sitting beside a
  // monorepo checkout. That last leg is why the plugin works before the
  // engine is packaged at all; in a copy installed from git it simply is
  // not there, and the search falls through to nothing.
  //
  // The plugin folder is resolved through `readlink -f` because it is
  // normally reached as a symlink out of ~/.config/omarchy/plugins:
  // without that, `..` would climb into the plugins directory instead of
  // the repo.
  readonly property string pluginDir: String(Qt.resolvedUrl(".")).replace(/^file:\/\//, "").replace(/\/$/, "")

  function findEngine() {
    if (locate.running) {
      return
    }
    locate.command = [
      "sh", "-c",
      'dir=$(readlink -f "$1" 2>/dev/null || printf %s "$1"); shift;' +
      ' if command -v sinopia >/dev/null 2>&1; then command -v sinopia; exit 0; fi;' +
      ' for c in "$@" "$dir/../target/release/sinopia" "$dir/../target/debug/sinopia"; do' +
      '   [ -n "$c" ] && [ -x "$c" ] && { printf %s\\\\n "$c"; exit 0; };' +
      ' done; exit 0',
      "sh",
      root.pluginDir,
      Quickshell.env("HOME") + "/.local/bin/sinopia",
      String(root.setting("enginePath", ""))
    ]
    locate.running = true
  }

  Process {
    id: locate
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        root.enginePath = String(text || "").trim()
        root.searched = true
      }
    }
  }

  // ----------------------------------------------------------------- intent

  // execDetached, never Process: the project outlives the popout that
  // opened it, and the shell must not become the owner of a wgpu process.
  function launch(args) {
    if (!root.haveEngine) {
      return
    }
    Quickshell.execDetached([root.enginePath].concat(args))
    root.close()
  }

  function newBoard() { root.launch(["--new"]) }

  // A draft is opened by id and a file by path — two flags, because the
  // engine's schema is closed and will not guess which a string is.
  function openProject(p) {
    if (!p) {
      return
    }
    if (p.path !== "") {
      root.launch(["--open-file", p.path])
    } else if (root.validId(p.id)) {
      root.launch(["--open", p.id])
    }
  }

  // ------------------------------------------------------------ the screens

  // The list is a chooser, so it arrives with its first row already
  // picked and showing it. The menu does not: its two rows carry their
  // own letters, and a cursor nobody can see is one nobody meant to act
  // on.
  function toScreen(name) {
    root.screen = name
    root.cursor = 0
    root.filter = ""
    root.cursorLive = (name === "open")
  }

  function setFilter(next) {
    root.filter = next
    root.cursor = 0
    root.cursorLive = true
  }

  // Out one layer at a time: the filter first, since a typed word is
  // work, then the screen, then the popout.
  function back() {
    if (root.screen === "open" && root.filter !== "") {
      root.setFilter("")
    } else if (root.screen === "open") {
      root.toScreen("menu")
    } else {
      root.close()
    }
  }

  function move(delta) {
    var n = root.rowCount
    if (n <= 0) {
      return
    }
    root.cursorLive = true
    root.cursor = (root.cursor + delta + n) % n
  }

  // Never act on a selection the panel is not showing. Enter reaches this
  // from the moment the popout maps, and with a cursor parked on row 0
  // unseen that opened a board nobody asked for — once, out of a stray
  // keystroke, and the board it wrote is indistinguishable from a real
  // one. The arrows raise the cursor; until they do, the letters are the
  // only way in, and they say which row they mean.
  function activate() {
    if (!root.cursorLive) {
      return
    }
    if (root.screen === "menu") {
      if (root.cursor === 0) {
        root.newBoard()
      } else {
        root.toScreen("open")
      }
      return
    }
    root.openProject(root.shown[root.cursor])
  }

  // One key handler rather than Ui/PanelKeyCatcher, which spends j, k, h,
  // l and x on navigation: on the list every printable key is the filter,
  // and a project called "planejamento" has to be typeable. The shape is
  // the shell's own — plugins/clipboard does the same for the same reason.
  function pressed(event) {
    var k = event.key
    var plain = !(event.modifiers & (Qt.ControlModifier | Qt.AltModifier | Qt.MetaModifier))

    if (k === Qt.Key_Tab || k === Qt.Key_Backtab) {
      root.switchPanel((event.modifiers & Qt.ShiftModifier) || k === Qt.Key_Backtab ? -1 : 1)
      event.accepted = true
      return
    }
    if (k === Qt.Key_Escape) {
      root.back()
      event.accepted = true
      return
    }
    if (k === Qt.Key_Down) {
      root.move(1)
      event.accepted = true
      return
    }
    if (k === Qt.Key_Up) {
      root.move(-1)
      event.accepted = true
      return
    }
    if (k === Qt.Key_Return || k === Qt.Key_Enter) {
      root.activate()
      event.accepted = true
      return
    }

    if (root.screen === "menu") {
      if (k === Qt.Key_Right && root.cursor === 1) {
        root.toScreen("open")
        event.accepted = true
        return
      }
      if (!plain) {
        return
      }
      var letter = String(event.text || "").toLowerCase()
      if (letter === "n") {
        root.newBoard()
        event.accepted = true
      } else if (letter === "o") {
        root.toScreen("open")
        event.accepted = true
      }
      return
    }

    // On the list, Left steps back out only while there is nothing typed:
    // once there is, it belongs to the word being written.
    if (k === Qt.Key_Left && root.filter === "") {
      root.toScreen("menu")
      event.accepted = true
      return
    }
    if (Util.editsFilter(event, root.filter)) {
      root.setFilter(Util.editedFilter(event, root.filter))
      event.accepted = true
      return
    }
    if (plain && event.text && event.text.length === 1 && event.text >= " ") {
      root.setFilter(root.filter + event.text)
      event.accepted = true
    }
  }

  onOpenedChanged: {
    if (!opened) {
      return
    }
    root.toScreen("menu")
    root.findEngine()
    index.reload()
  }

  // ---------------------------------------------------------------- bar item

  BarIconButton {
    id: item
    anchors.fill: parent
    bar: root.bar
    text: "󰏭"                   // nf-md-pencil_box_outline: a board drawn on
    slotSize: Style.bar.statusSlot
    fontSize: Style.font.caption

    tooltipText: "Sinopia"
      + (root.projects.length > 0
         ? "  ·  " + root.projects.length + (root.projects.length === 1 ? " project" : " projects")
         : "")

    onPressed: function (button) {
      // Right click is the shortest path to a blank board: the popout is
      // two keystrokes, this is none.
      if (button === Qt.RightButton) {
        if (!root.haveEngine) {
          root.findEngine()
        } else {
          root.newBoard()
        }
        return
      }
      root.toggle()
    }
  }

  // ------------------------------------------------------------------ popout

  KeyboardPanel {
    id: popout
    anchorItem: item
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: catcher
    contentWidth: popout.fittedContentWidth(Style.space(320))
    contentHeight: popout.fittedContentHeight(body.implicitHeight, Style.space(560))

    Item {
      id: catcher
      anchors.fill: parent
      focus: true
      Keys.priority: Keys.BeforeItem
      Keys.onPressed: function (event) { root.pressed(event) }

      Column {
        id: body
        width: parent.width
        spacing: Style.spacing.md

        PanelHero {
          width: parent.width
          foreground: root.foreground
          title: root.screen === "menu" ? "Sinopia" : "Recent projects"
          detail: root.screen === "open" && root.projects.length > 0
            ? (root.filter === ""
               ? String(root.projects.length)
               : root.shown.length + "/" + root.projects.length)
            : ""
          meta: root.screen === "menu"
            ? (root.searched && !root.haveEngine ? "engine not found" : "whiteboard")
            : (root.filter === "" ? "newest first — type to search" : "“" + root.filter + "”")
        }

        PanelSeparator {
          width: parent.width
          foreground: root.foreground
        }

        // The engine is missing: say so and say where it was looked for,
        // instead of leaving rows that swallow every click. There is no
        // package to offer yet, so nothing here pretends to install one.
        Column {
          width: parent.width
          spacing: Style.spacing.sm
          visible: root.searched && !root.haveEngine

          Text {
            width: parent.width
            textFormat: Text.PlainText
            text: "No sinopia on the PATH, in ~/.local/bin, or built in this checkout."
            color: root.foreground
            font.family: Style.font.family
            font.pixelSize: Style.font.bodySmall
            wrapMode: Text.WordWrap
          }

          Text {
            width: parent.width
            textFormat: Text.PlainText
            text: "cargo build in the repo, or set enginePath on this widget in shell.json."
            color: root.foreground
            opacity: 0.7
            font.family: Style.font.family
            font.pixelSize: Style.font.caption
            wrapMode: Text.WordWrap
          }
        }

        // --------------------------------------------------------- the menu

        Column {
          width: parent.width
          spacing: Style.spacing.sm
          visible: root.screen === "menu" && root.haveEngine

          ActionRow {
            width: parent.width
            rowIndex: 0
            glyph: "󰝒"
            label: "New board"
            hint: "n"
          }

          ActionRow {
            width: parent.width
            rowIndex: 1
            glyph: "󰉖"
            label: "Recent…"
            hint: "o"
            chevron: true
          }
        }

        // ----------------------------------------------------- the projects

        Text {
          width: parent.width
          visible: root.screen === "open" && root.shown.length === 0
          textFormat: Text.PlainText
          text: root.projects.length === 0
            ? "Nothing saved yet."
            : "Nothing matches “" + root.filter + "”."
          color: root.foreground
          opacity: 0.7
          font.family: Style.font.family
          font.pixelSize: Style.font.bodySmall
          wrapMode: Text.WordWrap
        }

        ListView {
          id: list
          width: parent.width
          visible: root.screen === "open" && root.shown.length > 0
          height: visible ? Math.min(contentHeight, Style.space(340)) : 0
          spacing: Style.spacing.sm
          clip: true
          boundsBehavior: Flickable.StopAtBounds
          interactive: contentHeight > height

          model: root.shown
          currentIndex: root.screen === "open" ? root.cursor : -1
          onCurrentIndexChanged: if (currentIndex >= 0) positionViewAtIndex(currentIndex, ListView.Contain)

          delegate: ProjectRow {
            required property var modelData
            required property int index
            width: ListView.view.width
            project: modelData
            rowIndex: index
          }
        }
      }
    }
  }

  // -------------------------------------------------------------- components

  // A menu row: the glyph, the label, and the key that reaches it without
  // the mouse. The pill is the whole point of the popout — it teaches the
  // shortcut that makes the popout unnecessary.
  component ActionRow: CursorSurface {
    id: action
    required property int rowIndex
    property string glyph: ""
    property string label: ""
    property string hint: ""
    property bool chevron: false

    hasCursor: root.cursorLive && root.screen === "menu" && root.cursor === rowIndex
    foreground: root.foreground
    implicitHeight: actionRow.implicitHeight + Style.spacing.rowPaddingX

    MouseArea {
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onContainsMouseChanged: if (containsMouse) {
        root.cursorLive = true
        root.cursor = action.rowIndex
      }
      onClicked: root.activate()
    }

    Row {
      id: actionRow
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.leftMargin: Style.spacing.rowPaddingX
      anchors.rightMargin: Style.spacing.rowPaddingX
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.spacing.controlGap

      Text {
        id: actionGlyph
        anchors.verticalCenter: parent.verticalCenter
        text: action.glyph
        color: root.foreground
        font.family: Style.font.family
        font.pixelSize: Style.font.icon
      }

      Text {
        anchors.verticalCenter: parent.verticalCenter
        width: actionRow.width - actionRow.spacing * 2 - actionGlyph.implicitWidth
          - pill.width - (action.chevron ? arrow.implicitWidth + actionRow.spacing : 0)
        textFormat: Text.PlainText
        text: action.label
        color: root.foreground
        font.family: Style.font.family
        font.pixelSize: Style.font.body
        elide: Text.ElideRight
      }

      Rectangle {
        id: pill
        anchors.verticalCenter: parent.verticalCenter
        width: Math.max(Style.space(18), hintText.implicitWidth + Style.spacing.md)
        height: Style.space(18)
        radius: Style.cornerRadius > 0 ? Style.cornerRadius : Style.space(4)
        color: Qt.rgba(root.foreground.r, root.foreground.g, root.foreground.b, 0.12)

        Text {
          id: hintText
          anchors.centerIn: parent
          textFormat: Text.PlainText
          text: action.hint
          color: root.foreground
          font.family: Style.font.family
          font.pixelSize: Style.font.caption
          font.bold: true
        }
      }

      Text {
        id: arrow
        anchors.verticalCenter: parent.verticalCenter
        visible: action.chevron
        text: "›"
        color: root.foreground
        opacity: 0.6
        font.family: Style.font.family
        font.pixelSize: Style.font.body
      }
    }
  }

  // A project row: the mark saying which kind it is, the name, and how
  // long ago it was written. A draft is called by its title and a file by
  // its own name, exactly as the tab calls each. Everything here came out
  // of `index.json`, so it is drawn as plain text and elided — the index
  // is the surface between two processes, and it is read as data, never
  // as markup (§6).
  component ProjectRow: CursorSurface {
    id: projectRow
    required property var project
    required property int rowIndex

    readonly property bool isFile: project.path !== ""
    readonly property bool gone: isFile && root.missing[project.path] === true

    hasCursor: root.cursorLive && root.screen === "open" && root.cursor === rowIndex
    foreground: root.foreground
    implicitHeight: projectLabels.implicitHeight + Style.spacing.rowPaddingX
    // A file that is not there is still worth showing — an unmounted
    // drive is not a deletion — but it is not worth promising.
    opacity: gone ? 0.45 : 1.0

    MouseArea {
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onContainsMouseChanged: if (containsMouse) {
        root.cursorLive = true
        root.cursor = projectRow.rowIndex
      }
      onClicked: root.openProject(projectRow.project)
    }

    Row {
      id: projectLabels
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.leftMargin: Style.spacing.rowPaddingX
      anchors.rightMargin: Style.spacing.rowPaddingX
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.spacing.controlGap

      Text {
        id: mark
        anchors.verticalCenter: parent.verticalCenter
        // The same two glyphs the menu uses, so the row says which door
        // it came through: a file the person named, or a draft the store
        // is keeping for them.
        text: projectRow.isFile ? "󰉖" : "󰝒"
        color: root.foreground
        opacity: 0.55
        font.family: Style.font.family
        font.pixelSize: Style.font.iconSmall
      }

      Text {
        anchors.verticalCenter: parent.verticalCenter
        width: Math.max(0, projectLabels.width - projectLabels.spacing * 2
          - mark.implicitWidth - stamp.implicitWidth)
        textFormat: Text.PlainText
        text: projectRow.project.name
        color: root.foreground
        opacity: projectRow.project.titled ? 1.0 : 0.7
        font.family: Style.font.family
        font.pixelSize: Style.font.body
        elide: Text.ElideRight
      }

      Text {
        id: stamp
        anchors.verticalCenter: parent.verticalCenter
        textFormat: Text.PlainText
        text: projectRow.gone ? "not there" : root.whenText(projectRow.project.updated)
        color: root.foreground
        opacity: 0.6
        font.family: Style.font.family
        font.pixelSize: Style.font.caption
      }
    }
  }
}
