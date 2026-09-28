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
}
