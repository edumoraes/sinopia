import QtQuick
import QtTest
import "../../plugin/recents.js" as Recents

// The plugin's reading of `index.json`, which is the surface between two
// processes and is read as untrusted data (§6). Run with
// `QT_QPA_PLATFORM=offscreen qmltestrunner -input tests/plugin` (Qt 6's).
TestCase {
  name: "Recents"

  function index(boards) {
    return JSON.stringify({ "schema": 1, "boards": boards })
  }

  function test_an_id_the_engine_would_refuse_is_not_valid() {
    verify(Recents.validId("01ABC_def-9"))
    verify(!Recents.validId(""))
    verify(!Recents.validId("../x"))
    verify(!Recents.validId("a b"))
    verify(!Recents.validId("x".repeat(65)))
    verify(Recents.validId("x".repeat(64)))
  }

  function test_a_file_is_called_by_its_own_name() {
    compare(Recents.fileName("/home/you/plan.sinopia"), "plan")
    compare(Recents.fileName("/home/you/.hidden"), ".hidden")
    compare(Recents.fileName("/home/you/notes"), "notes")
    compare(Recents.fileName("/home/you/"), "untitled")
  }

  function test_the_recents_come_newest_first() {
    var list = Recents.parse(index([
      { "id": "old", "title": "Old", "updated_at": 100, "thumb": null },
      { "id": "new", "title": "New", "updated_at": 200, "thumb": null }
    ]))
    compare(list.length, 2)
    compare(list[0].id, "new")
    compare(list[1].id, "old")
    compare(list[0].updated, 200)
  }

  function test_a_draft_is_called_by_its_title_and_a_file_by_its_name() {
    var list = Recents.parse(index([
      { "id": "a", "title": "  auth \n flow ", "updated_at": 2, "thumb": null },
      { "id": "b", "title": "inside", "updated_at": 1, "thumb": null, "path": "/w/plan.sinopia" }
    ]))
    compare(list[0].name, "auth flow")
    compare(list[0].path, "")
    verify(list[0].titled)
    compare(list[1].name, "plan")
    compare(list[1].path, "/w/plan.sinopia")
  }

  function test_a_draft_without_a_title_is_untitled() {
    var list = Recents.parse(index([{ "id": "a", "title": "", "updated_at": 1, "thumb": null }]))
    compare(list[0].name, "untitled")
    verify(!list[0].titled)
  }

  function test_an_entry_with_an_id_the_engine_refuses_is_left_out() {
    var list = Recents.parse(index([
      { "id": "../etc", "title": "x", "updated_at": 1, "thumb": null },
      { "id": "ok", "title": "y", "updated_at": 1, "thumb": null }
    ]))
    compare(list.length, 1)
    compare(list[0].id, "ok")
  }

  function test_an_index_that_will_not_parse_answers_nothing() {
    // The caller keeps the last good list up: a reader can meet a
    // truncated read.
    compare(Recents.parse("{ \"schema\": 1, \"boa"), null)
  }

  function test_an_empty_index_is_an_empty_list() {
    compare(Recents.parse(index([])).length, 0)
    compare(Recents.parse("{}").length, 0)
  }

  function test_a_recent_has_a_thumbnail_when_the_index_names_its_own() {
    var list = Recents.parse(index([
      { "id": "a", "title": "t", "updated_at": 2, "thumb": "thumbs/a.png" },
      { "id": "b", "title": "t", "updated_at": 1, "thumb": null }
    ]))
    verify(list[0].thumb)
    verify(!list[1].thumb)
  }

  function test_a_thumbnail_anywhere_but_its_own_place_is_not_taken() {
    // §6: a preview is read only from the canonical thumbs/<id>.png. The
    // index is untrusted, so a path it names anywhere else is not a
    // picture, it is a request to read a file.
    var list = Recents.parse(index([
      { "id": "a", "title": "t", "updated_at": 5, "thumb": "thumbs/b.png" },
      { "id": "b", "title": "t", "updated_at": 4, "thumb": "/etc/passwd" },
      { "id": "c", "title": "t", "updated_at": 3, "thumb": "thumbs/../../c.png" },
      { "id": "d", "title": "t", "updated_at": 2, "thumb": "thumbs/d.svg" },
      { "id": "e", "title": "t", "updated_at": 1, "thumb": 7 }
    ]))
    compare(list.length, 5)
    for (var i = 0; i < list.length; i++) {
      verify(!list[i].thumb, list[i].id)
    }
  }

  function test_a_thumbnail_s_source_is_built_from_the_id_alone() {
    var p = Recents.parse(index([
      { "id": "a1", "title": "t", "updated_at": 42, "thumb": "thumbs/a1.png" }
    ]))[0]
    compare(Recents.thumbSource("/home/you/.local/share/sinopia", p),
            "file:///home/you/.local/share/sinopia/thumbs/a1.png?v=42")
  }

  function test_a_new_save_is_a_new_source() {
    // The image cache keys on the URL: without the save time on it, the
    // row would go on showing the picture it first loaded.
    var dir = "/d"
    var before = Recents.parse(index([{ "id": "a", "title": "t", "updated_at": 1, "thumb": "thumbs/a.png" }]))[0]
    var after = Recents.parse(index([{ "id": "a", "title": "t", "updated_at": 2, "thumb": "thumbs/a.png" }]))[0]
    verify(Recents.thumbSource(dir, before) !== Recents.thumbSource(dir, after))
  }

  function test_a_recent_without_a_thumbnail_has_no_source() {
    var p = Recents.parse(index([{ "id": "a", "title": "t", "updated_at": 1, "thumb": null }]))[0]
    compare(Recents.thumbSource("/d", p), "")
    compare(Recents.thumbSource("/d", null), "")
  }
}
