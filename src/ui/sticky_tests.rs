use super::*;

#[test]
fn window_title_embeds_the_note_id() {
    let title = window_title("18f2a1b3c4d-0001");
    assert_eq!(title, "Beamer Note 18f2a1b3c4d-0001");
    assert!(
        title.starts_with(TITLE_PREFIX),
        "the GNOME extension matches on this prefix; changing it breaks placement"
    );
}

#[test]
fn titles_are_unique_per_note() {
    assert_ne!(window_title("a"), window_title("b"));
}

#[test]
fn a_resize_at_scale_two_stores_half_the_physical_numbers() {
    // Invisible on this machine, where everything is scale 1.0, and wrong on
    // every HiDPI display, reopening each note at double size.
    assert_eq!(logical_size((800, 640), 2.0), Some((400, 320)));
    assert_eq!(logical_size((400, 320), 1.0), Some((400, 320)));
    assert_eq!(logical_size((600, 480), 1.5), Some((400, 320)));
}

#[test]
fn a_zero_sized_resize_is_ignored() {
    // A minimize on some compositors. Storing it would reopen the note as a
    // sliver with no way back to a usable size.
    assert_eq!(logical_size((0, 640), 2.0), None);
    assert_eq!(logical_size((800, 0), 2.0), None);
    assert_eq!(
        logical_size((800, 640), 0.0),
        None,
        "a bad scale must not divide by zero"
    );
}

#[test]
fn an_empty_note_gets_room_and_a_long_one_is_capped() {
    assert_eq!(rows_for(""), 3, "an empty note should not be a one-line slot");
    assert_eq!(rows_for("one"), 3);
    assert_eq!(rows_for("a\nb\nc\nd"), 4);
    assert_eq!(
        rows_for(&"x\n".repeat(200)),
        40,
        "a runaway note must not make a window of unbounded height"
    );
}

#[test]
fn a_trailing_enter_grows_the_textarea() {
    // `str::lines` drops the trailing empty line, so the textarea never
    // grew on an Enter at the end of the note.
    assert_eq!(rows_for("hello\n"), 3);
    assert_eq!(rows_for("a\nb\nc\nd\n"), 5);
}
