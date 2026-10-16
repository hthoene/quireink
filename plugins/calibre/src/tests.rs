use quireink_i18n::{Catalog, check};

#[test]
fn translations_are_complete() {
    let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
    let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
    let issues = check::compare(&en, &de);
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn calibres_metadata_for_the_library() {
    let metadata = r#"{"title": "Alice", "authors": ["Lewis Carroll", "John Tenniel"],
        "series": "Alice", "series_index": 1.0, "tags": ["Classics", "Kids"], "lpath": "a.txt"}"#;
    let p = super::provided(metadata);
    assert_eq!(p.title.as_deref(), Some("Alice"));
    assert_eq!(p.author.as_deref(), Some("Lewis Carroll"));
    assert_eq!(p.series_index.as_deref(), Some("1"));
    assert_eq!(p.tags, ["Classics", "Kids"]);
    let p = super::provided(r#"{"title": "B", "series": null, "series_index": null}"#);
    assert_eq!((p.series, p.series_index), (None, None));
}
