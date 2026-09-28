.pragma library

// The recents as the plugin reads them out of `index.json` — the one
// surface between the shell and the engine (§6), read as untrusted data.
// Pure, so it can be tested without a shell: tests/plugin/tst_recents.qml.

// Mirrors `validate_id` in the engine's own store: a row naming an id it
// would refuse could never be opened, so it is not shown.
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

// Every recent project in `raw`, newest first: a draft, which the store
// keeps and which is opened by id, or a file the person named, which is
// opened by path. `null` for a text that will not parse — the engine
// writes atomically, but a reader can still meet a truncated read, and
// the caller keeps the last good list up.
function parse(raw) {
  var list = []
  try {
    var d = JSON.parse(String(raw || ""))
    var entries = (d && d.boards) || []
    for (var i = 0; i < entries.length; i++) {
      var e = entries[i]
      if (!e || !validId(e.id)) {
        continue
      }
      var path = typeof e.path === "string" && e.path !== "" ? e.path : ""
      var title = String(e.title || "").replace(/\s+/g, " ").trim()
      list.push({
        "id": String(e.id),
        "path": path,
        "name": path !== "" ? fileName(path) : (title !== "" ? title : "untitled"),
        "titled": path !== "" || title !== "",
        "updated": Number(e.updated_at) || 0
      })
    }
    list.sort(function (a, b) { return b.updated - a.updated })
  } catch (err) {
    return null
  }
  return list
}
