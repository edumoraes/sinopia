import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

// Omawhite on the bar: an icon whose popout holds the two ways into the
// board — a new one, or one of those already on disk.
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
  moduleName: "edu.omawhite"
  ipcTarget: "edu.omawhite"

  // The bar item follows the Panel's foreground (transparency-aware); the
  // popout's content follows the theme's popup surface.
  readonly property color foreground: Color.popups.text

  // "menu" | "open". The second screen is the board list, and Esc there
  // steps back to the first rather than closing the popout — a drill-in
  // that shut the whole thing on its way out would be a trap.
  property string screen: "menu"

  // The mirror of index.json. A title is untrusted text (§6): it is drawn
  // as plain text and elided, never as markup, and an id is only allowed
  // through to argv after it matches what the engine itself accepts.
  property var boards: []

  // The engine, resolved when the popout opens (§10.1). `searched` is what
  // separates "nobody has answered yet" from "it is not installed" — the
  // rows stay live for the first and go dim for the second.
  property string enginePath: ""
  property bool searched: false
  readonly property bool haveEngine: enginePath !== ""

  // One cursor for whichever screen is up. `cursorLive` is what keeps a
  // single highlight on screen: the keyboard raises it, and the mouse
  // moves it rather than painting a second one (the CursorSurface
  // contract).
  property int cursor: 0
  property bool cursorLive: false

  readonly property int rowCount: screen === "menu" ? 2 : boards.length

  implicitWidth: item.implicitWidth
  implicitHeight: item.implicitHeight

  // -------------------------------------------------------------- the index

  // A board that does not name an id the engine would accept is dropped
  // rather than shown: a row that cannot be opened is worse than no row.
  function validId(id) {
    var s = String(id || "")
    return s.length > 0 && s.length <= 64 && /^[A-Za-z0-9_-]+$/.test(s)
  }

  function absorb(raw) {
    var list = []
    try {
      var d = JSON.parse(String(raw || ""))
      var entries = (d && d.boards) || []
      for (var i = 0; i < entries.length; i++) {
        var e = entries[i]
        if (!e || !root.validId(e.id)) continue
        list.push({
          "id": String(e.id),
          "title": String(e.title || "").replace(/\s+/g, " ").trim(),
          "updated": Number(e.updated_at) || 0
        })
      }
      list.sort(function (a, b) { return b.updated - a.updated })
    } catch (err) {
      // A half-written index (the engine writes atomically, but a reader can
      // still meet a truncated read) leaves the last good list standing.
      return
    }
    root.boards = list
  }

  function whenText(stamp) {
    var t = Number(stamp)
    if (!isFinite(t) || t <= 0) return ""
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
    path: Quickshell.env("HOME") + "/.local/share/omawhite/index.json"
    watchChanges: true
    printErrors: false          // no boards yet is a state, not an error
    onLoaded: root.absorb(text())
    onLoadFailed: root.boards = []
    onFileChanged: reload()
  }

  // -------------------------------------------------------------- the engine

  // §10.1's search, in order: `omawhite` on the PATH, then ~/.local/bin,
  // then the configurable path, then the dev build sitting beside a
  // monorepo checkout. That last leg is why the plugin works before the
  // engine is packaged at all; in a copy installed from git it simply is
  // not there, and the search falls through to nothing.
  //
  // The plugin folder is resolved through `readlink -f` because it is
  // normally reached as a symlink out of ~/.config/omarchy/plugins: without
  // that, `..` would climb into the plugins directory instead of the repo.
  readonly property string pluginDir: String(Qt.resolvedUrl(".")).replace(/^file:\/\//, "").replace(/\/$/, "")

  function findEngine() {
    if (locate.running) return
    locate.command = [
      "sh", "-c",
      'dir=$(readlink -f "$1" 2>/dev/null || printf %s "$1"); shift;' +
      ' if command -v omawhite >/dev/null 2>&1; then command -v omawhite; exit 0; fi;' +
      ' for c in "$@" "$dir/../target/release/omawhite" "$dir/../target/debug/omawhite"; do' +
      '   [ -n "$c" ] && [ -x "$c" ] && { printf %s\\\\n "$c"; exit 0; };' +
      ' done; exit 0',
      "sh",
      root.pluginDir,
      Quickshell.env("HOME") + "/.local/bin/omawhite",
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

  // ------------------------------------------------------------------ intent

  // execDetached, never Process: the board outlives the popout that opened
  // it, and the shell must not become the owner of a wgpu process.
  function launch(args) {
    if (!root.haveEngine) return
    Quickshell.execDetached([root.enginePath].concat(args))
    root.close()
  }

  function newBoard() { root.launch(["--new"]) }

  function openBoard(id) {
    if (!root.validId(id)) return
    root.launch(["--open", String(id)])
  }

  // ----------------------------------------------------------- the two screens

  // The list is a chooser, so it arrives with its first row already picked
  // and showing it. The menu does not: its two rows carry their own letters,
  // and a cursor nobody can see is one nobody meant to act on.
  function toScreen(name) {
    root.screen = name
    root.cursor = 0
    root.cursorLive = (name === "open")
  }

  function back() {
    if (root.screen === "menu") root.close()
    else root.toScreen("menu")
  }

  function move(delta) {
    var n = root.rowCount
    if (n <= 0) return
    root.cursorLive = true
    root.cursor = (root.cursor + delta + n) % n
  }

  // Never act on a selection the panel is not showing. Enter and Space reach
  // the catcher from the moment the popout maps, and with a cursor parked on
  // row 0 unseen that opened a board nobody asked for — once, out of a stray
  // keystroke, and the board it wrote is indistinguishable from a real one.
  // The arrows are what raise the cursor; until they do, the letters are the
  // only way in, and they say which row they mean.
  function activate() {
    if (!root.cursorLive) return
    if (root.screen === "menu") {
      if (root.cursor === 0) root.newBoard()
      else root.toScreen("open")
      return
    }
    var b = root.boards[root.cursor]
    if (b) root.openBoard(b.id)
  }

  function typed(key) {
    var k = String(key || "").toLowerCase()
    if (k === "n") { root.newBoard(); return }
    if (k === "o") { root.toScreen("open"); return }
  }

  onOpenedChanged: {
    if (!opened) return
    root.toScreen("menu")
    root.findEngine()
    index.reload()
  }

  // --------------------------------------------------------------- bar item

  BarIconButton {
    id: item
    anchors.fill: parent
    bar: root.bar
    text: "󰏭"                   // nf-md-pencil_box_outline: a board drawn on
    slotSize: Style.bar.statusSlot
    fontSize: Style.font.caption

    tooltipText: "Omawhite"
      + (root.boards.length > 0 ? "  ·  " + root.boards.length + (root.boards.length === 1 ? " board" : " boards") : "")

    onPressed: function (button) {
      // Right click is the shortest path to a blank board: the popout is
      // two keystrokes, this is none.
      if (button === Qt.RightButton) {
        if (!root.haveEngine) root.findEngine()
        else root.newBoard()
        return
      }
      root.toggle()
    }
  }

  // ---------------------------------------------------------------- popout

  KeyboardPanel {
    id: popout
    anchorItem: item
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: catcher
    contentWidth: popout.fittedContentWidth(Style.space(300))
    contentHeight: popout.fittedContentHeight(body.implicitHeight, Style.space(560))

    PanelKeyCatcher {
      id: catcher
      anchors.fill: parent

      onCloseRequested: root.back()
      onTabRequested: function (direction) { root.switchPanel(direction) }
      onMoveRequested: function (dx, dy) {
        if (dy !== 0) { root.move(dy); return }
        // Right walks into the list, left walks back out — the same axis the
        // chevron on the row promises.
        if (dx > 0 && root.screen === "menu" && root.cursor === 1) root.toScreen("open")
        else if (dx < 0 && root.screen === "open") root.toScreen("menu")
      }
      onActivateRequested: root.activate()
      onTextKey: function (t) { root.typed(t) }

      Column {
        id: body
        width: parent.width
        spacing: Style.spacing.md

        PanelHero {
          width: parent.width
          foreground: root.foreground
          title: root.screen === "menu" ? "Omawhite" : "Open a board"
          detail: root.screen === "open" && root.boards.length > 0 ? String(root.boards.length) : ""
          meta: root.screen === "menu"
            ? (root.searched && !root.haveEngine ? "engine not found" : "whiteboard")
            : "newest first"
        }

        PanelSeparator {
          width: parent.width
          foreground: root.foreground
        }

        // The engine is missing: say so and say where it is looked for,
        // instead of leaving two rows that swallow every click. There is no
        // package to offer yet, so nothing here pretends to install one.
        Column {
          width: parent.width
          spacing: Style.spacing.sm
          visible: root.searched && !root.haveEngine

          Text {
            width: parent.width
            textFormat: Text.PlainText
            text: "No omawhite on the PATH, in ~/.local/bin, or built in this checkout."
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

        // ------------------------------------------------------ the menu

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
            label: "Open…"
            hint: "o"
            chevron: true
          }
        }

        // ------------------------------------------------------ the boards

        Text {
          width: parent.width
          visible: root.screen === "open" && root.boards.length === 0
          textFormat: Text.PlainText
          text: "No boards saved yet."
          color: root.foreground
          opacity: 0.7
          font.family: Style.font.family
          font.pixelSize: Style.font.bodySmall
        }

        ListView {
          id: list
          width: parent.width
          visible: root.screen === "open" && root.boards.length > 0
          height: visible ? Math.min(contentHeight, Style.space(320)) : 0
          spacing: Style.spacing.sm
          clip: true
          boundsBehavior: Flickable.StopAtBounds
          interactive: contentHeight > height

          model: root.boards
          currentIndex: root.screen === "open" ? root.cursor : -1
          onCurrentIndexChanged: if (currentIndex >= 0) positionViewAtIndex(currentIndex, ListView.Contain)

          delegate: BoardRow {
            required property var modelData
            required property int index
            width: ListView.view.width
            board: modelData
            rowIndex: index
          }
        }
      }
    }
  }

  // ------------------------------------------------------------- components

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

  // A board row: the title as the engine wrote it, and how long ago it was
  // touched. Both are plain text — the index is the surface between two
  // processes, and it is read as data, never as markup (§6).
  component BoardRow: CursorSurface {
    id: boardRow
    required property var board
    required property int rowIndex

    hasCursor: root.cursorLive && root.screen === "open" && root.cursor === rowIndex
    foreground: root.foreground
    implicitHeight: boardLabels.implicitHeight + Style.spacing.rowPaddingX

    MouseArea {
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onContainsMouseChanged: if (containsMouse) {
        root.cursorLive = true
        root.cursor = boardRow.rowIndex
      }
      onClicked: root.openBoard(boardRow.board.id)
    }

    Row {
      id: boardLabels
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.leftMargin: Style.spacing.rowPaddingX
      anchors.rightMargin: Style.spacing.rowPaddingX
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.spacing.controlGap

      Text {
        anchors.verticalCenter: parent.verticalCenter
        width: boardLabels.width - boardLabels.spacing - stamp.implicitWidth
        textFormat: Text.PlainText
        text: boardRow.board.title !== "" ? boardRow.board.title : "untitled"
        color: root.foreground
        opacity: boardRow.board.title !== "" ? 1.0 : 0.7
        font.family: Style.font.family
        font.pixelSize: Style.font.body
        elide: Text.ElideRight
      }

      Text {
        id: stamp
        anchors.verticalCenter: parent.verticalCenter
        textFormat: Text.PlainText
        text: root.whenText(boardRow.board.updated)
        color: root.foreground
        opacity: 0.6
        font.family: Style.font.family
        font.pixelSize: Style.font.caption
      }
    }
  }
}
