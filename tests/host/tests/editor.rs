use wisp_host_tests::editor::{Editor, MAX_SOURCE_BYTES};
use wisp_host_tests::line_editor::{
    KEY_DELETE, KEY_DOWN, KEY_END, KEY_HOME, KEY_LEFT, KEY_RIGHT, KEY_UP,
};

fn keys(editor: &mut Editor, input: &[u8]) {
    for &key in input {
        editor.handle_key(key).unwrap();
    }
}

#[test]
fn edits_split_and_join_lines_in_the_middle() {
    let mut editor = Editor::new("alpha\nbeta").unwrap();
    keys(&mut editor, &[KEY_END, KEY_LEFT, KEY_LEFT, 0x0d]);
    assert_eq!(editor.source(), "alp\nha\nbeta");
    assert_eq!(editor.position(), (1, 0));
    keys(&mut editor, &[0x08]);
    assert_eq!(editor.source(), "alpha\nbeta");
    assert_eq!(editor.position(), (0, 3));
    keys(&mut editor, &[KEY_END, KEY_DELETE, b'!']);
    assert_eq!(editor.source(), "alpha!beta");
    assert_eq!(editor.line_count(), 1);
}

#[test]
fn vertical_navigation_remembers_column_across_short_lines() {
    let mut editor = Editor::new("abcdef\nx\nuvwxyz\n").unwrap();
    keys(
        &mut editor,
        &[KEY_RIGHT, KEY_RIGHT, KEY_RIGHT, KEY_RIGHT, KEY_DOWN],
    );
    assert_eq!(editor.position(), (1, 1));
    keys(&mut editor, &[KEY_DOWN]);
    assert_eq!(editor.position(), (2, 4));
    keys(&mut editor, &[KEY_DOWN]);
    assert_eq!(editor.position(), (3, 0));
    editor.move_vertical(-2);
    assert_eq!(editor.position(), (1, 1));
    keys(&mut editor, &[KEY_UP]);
    assert_eq!(editor.position(), (0, 4));
    keys(&mut editor, &[KEY_HOME, KEY_DOWN]);
    assert_eq!(editor.position(), (1, 0));
}

#[test]
fn jumps_and_pages_clamp_at_both_ends() {
    let mut editor = Editor::new("one\ntwo\nthree").unwrap();
    editor.go_to_line(usize::MAX);
    assert_eq!(editor.position(), (2, 0));
    keys(&mut editor, &[KEY_END]);
    editor.move_vertical(isize::MIN);
    assert_eq!(editor.position(), (0, 3));
    editor.move_vertical(isize::MAX);
    assert_eq!(editor.position(), (2, 5));
    editor.go_to_line(0);
    assert_eq!(editor.position(), (0, 0));
}

#[test]
fn utf8_navigation_and_deletion_preserve_character_boundaries() {
    let mut editor = Editor::new("é🙂z\n\t東京\r\n").unwrap();
    assert_eq!(editor.source(), "é🙂z\n\t東京\r\n");
    keys(&mut editor, &[KEY_RIGHT, b'!', KEY_RIGHT]);
    assert_eq!(editor.position(), (0, 3));
    keys(&mut editor, &[0x08]);
    assert_eq!(editor.source(), "é!z\n\t東京\r\n");
    keys(&mut editor, &[KEY_HOME, KEY_DELETE]);
    assert_eq!(editor.source(), "!z\n\t東京\r\n");
    keys(&mut editor, &[KEY_DOWN, KEY_RIGHT, KEY_DELETE]);
    assert_eq!(editor.source(), "!z\n\t京\r\n");
    assert_eq!(editor.position(), (1, 1));
}

#[test]
fn enter_and_tab_indent_at_the_cursor() {
    let mut editor = Editor::new("    if true {").unwrap();
    keys(&mut editor, &[KEY_END, 0x0d]);
    assert_eq!(editor.source(), "    if true {\n        ");
    assert_eq!(editor.position(), (1, 8));
    keys(&mut editor, b"print(1);");
    keys(&mut editor, &[0x0d]);
    assert!(editor.source().ends_with("print(1);\n        "));
    let mut editor = Editor::new("a").unwrap();
    keys(&mut editor, &[KEY_END, 0x09]);
    assert_eq!(editor.source(), "a   ");
    keys(&mut editor, &[0x09]);
    assert_eq!(editor.source(), "a       ");
}

#[test]
fn dirty_state_tracks_the_actual_saved_contents() {
    let mut editor = Editor::new("hello").unwrap();
    assert!(!editor.is_dirty());
    keys(&mut editor, &[KEY_END, b'!']);
    assert!(editor.is_dirty());
    keys(&mut editor, &[0x08]);
    assert!(!editor.is_dirty());
    keys(&mut editor, &[b'?']);
    editor.mark_saved();
    assert!(!editor.is_dirty());
    keys(&mut editor, &[KEY_HOME, KEY_DELETE]);
    assert!(editor.is_dirty());
    keys(&mut editor, &[b'h']);
    assert!(!editor.is_dirty());
}

#[test]
fn source_limit_rejects_open_and_insert_without_truncating() {
    assert!(Editor::new(&"a".repeat(MAX_SOURCE_BYTES + 1)).is_err());
    let original = "a".repeat(MAX_SOURCE_BYTES);
    let mut editor = Editor::new(&original).unwrap();
    keys(&mut editor, &[KEY_END]);
    for key in [b'b', 0x09, 0x0d] {
        assert!(editor.handle_key(key).is_err());
        assert_eq!(editor.source(), original);
        assert_eq!(editor.position(), (0, MAX_SOURCE_BYTES));
        assert!(!editor.is_dirty());
    }
    keys(&mut editor, &[0x08, b'b']);
    assert_eq!(editor.source().len(), MAX_SOURCE_BYTES);
    assert!(editor.source().ends_with('b'));
}

#[test]
fn autoindent_is_atomic_when_only_the_newline_would_fit() {
    let original = format!("    {}{{", "a".repeat(MAX_SOURCE_BYTES - 6));
    assert_eq!(original.len(), MAX_SOURCE_BYTES - 1);
    let mut editor = Editor::new(&original).unwrap();
    keys(&mut editor, &[KEY_END]);
    assert!(editor.handle_key(0x0d).is_err());
    assert_eq!(editor.source(), original);
    assert_eq!(editor.position(), (0, original.len()));
    assert!(!editor.is_dirty());
}

#[test]
fn empty_buffer_boundary_keys_and_unknown_commands_are_harmless() {
    let mut editor = Editor::new("").unwrap();
    keys(
        &mut editor,
        &[
            KEY_UP, KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_HOME, KEY_END, KEY_DELETE, 0x08, 0x1b, 0x90,
            0xff,
        ],
    );
    assert_eq!(editor.source(), "");
    assert_eq!(editor.position(), (0, 0));
    assert_eq!(editor.line_count(), 1);
    assert!(!editor.is_dirty());
}

#[test]
fn search_next_finds_forward_and_wraps_around() {
    let mut editor = Editor::new("let a = 1;\nlet b = 2;\nlet a = 3;\n").unwrap();
    assert!(editor.search_next("let b"));
    assert_eq!(editor.position(), (1, 0));
    assert!(editor.search_next("let a"));
    assert_eq!(editor.position(), (2, 0));
    // Wraps around to line 0.
    assert!(editor.search_next("let a"));
    assert_eq!(editor.position(), (0, 0));
    assert!(!editor.search_next("nonexistent"));
}
