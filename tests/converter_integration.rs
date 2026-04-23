use std::fs;
use std::path::Path;

use pretty_assertions::assert_eq;
use webvtt_to_md::{
    convert_bytes_to_markdown, convert_text_to_markdown, decode_transcript_bytes,
    detect_generation_source, parse_webvtt,
};

fn fixtures_dir() -> &'static Path {
    Path::new("tests/fixtures")
}

fn read_utf8(path: &str) -> String {
    fs::read_to_string(fixtures_dir().join(path)).expect("fixture must exist")
}

fn read_utf8_sig(path: &str) -> String {
    let bytes = fs::read(fixtures_dir().join(path)).expect("fixture must exist");
    let text = String::from_utf8(bytes).expect("fixture must be utf-8");
    text.trim_start_matches('\u{feff}').to_owned()
}

#[test]
fn transcript_to_markdown_matches_utf8_golden() {
    let transcript = read_utf8("input_utf8_nobom.vtt");
    let expected = read_utf8_sig("converted_utf8.md");
    let markdown = convert_text_to_markdown(&transcript, None).expect("conversion should succeed");
    assert_eq!(markdown, expected);
}

#[test]
fn generic_inline_tags_fixture_matches() {
    let transcript = read_utf8("input_generic_inline_tags.vtt");
    let expected = read_utf8("converted_generic_inline_tags.md");
    let markdown = convert_text_to_markdown(&transcript, Some("generic_webvtt"))
        .expect("conversion should succeed");
    assert_eq!(markdown, expected);
}

#[test]
fn generic_tag_combinations_fixture_matches() {
    let transcript = read_utf8("input_generic_tag_combinations.vtt");
    let expected = read_utf8("converted_generic_tag_combinations.md");
    let markdown = convert_text_to_markdown(&transcript, Some("generic_webvtt"))
        .expect("conversion should succeed");
    assert_eq!(markdown, expected);
}

#[test]
fn generic_html_entities_fixture_matches() {
    let transcript = read_utf8("input_generic_html_entities.vtt");
    let expected = read_utf8("converted_generic_html_entities.md");
    let markdown = convert_text_to_markdown(&transcript, Some("generic_webvtt"))
        .expect("conversion should succeed");
    assert_eq!(markdown, expected);
}

#[test]
fn style_only_fixture_matches() {
    let transcript = read_utf8("input_style_only.vtt");
    let expected = read_utf8("converted_style_only.md");
    let markdown = convert_text_to_markdown(&transcript, Some("generic_webvtt"))
        .expect("conversion should succeed");
    assert_eq!(markdown, expected);
}

#[test]
fn interleaved_teams_fixture_matches() {
    let transcript = read_utf8("input_ms_teams_scheduled_interleaved.vtt");
    let expected = read_utf8("converted_ms_teams_scheduled_interleaved.md");
    let markdown = convert_text_to_markdown(&transcript, Some("ms_teams_scheduled"))
        .expect("conversion should succeed");
    assert_eq!(markdown, expected);
}

#[test]
fn encoding_fixtures_decode_and_convert() {
    let fixtures = [
        ("input_utf8_nobom.vtt", "converted_utf8.md"),
        ("input_utf8_bom.vtt", "converted_utf8.md"),
        ("input_utf16_nobom.vtt", "converted_utf16.md"),
        ("input_utf16_bom.vtt", "converted_utf16.md"),
        ("input_utf16be_nobom.vtt", "converted_utf16be.md"),
        ("input_utf16be_bom.vtt", "converted_utf16be.md"),
        ("input_cp1254.vtt", "converted_cp1254.md"),
        ("input_cp1251.vtt", "converted_cp1251.md"),
    ];

    for (input_name, output_name) in fixtures {
        let input = fs::read(fixtures_dir().join(input_name)).expect("input fixture must exist");
        let expected = read_utf8_sig(output_name);
        let markdown = convert_bytes_to_markdown(&input, None, false, None)
            .expect("conversion should succeed");
        assert_eq!(markdown, expected, "fixture {input_name} mismatch");
    }
}

#[test]
fn strict_mode_rejects_wrong_explicit_charset() {
    let input = fs::read(fixtures_dir().join("input_invalid_utf8_cp1254.vtt"))
        .expect("input fixture must exist");
    let result = decode_transcript_bytes(&input, Some("utf-8"), false);
    assert!(result.is_err());
}

#[test]
fn lazy_mode_marks_decoding_errors() {
    let input = fs::read(fixtures_dir().join("input_invalid_utf8_cp1254.vtt"))
        .expect("input fixture must exist");
    let result =
        decode_transcript_bytes(&input, Some("utf-8"), true).expect("lazy decoding should succeed");
    assert!(result.contains("[ENC?]"));
}

#[test]
fn detection_matches_zoom_scheduled_case() {
    let transcript = "WEBVTT\n\n1\n00:00:01.000 --> 00:00:02.000\nSpeaker 1: Hello\n\n2\n00:00:02.000 --> 00:00:03.000\nSpeaker 2: Hi\n";
    let document = parse_webvtt(transcript).expect("parse should succeed");
    let source = detect_generation_source(&document).expect("detection should succeed");
    assert_eq!(source, "zoom_scheduled");
}

#[test]
fn blueprint_way_forward_fixture_matches() {
    let input = fs::read("Blueprint_way_forward.vtt").expect("input fixture must exist");
    let expected = read_utf8("converted_blueprint_way_forward.md");
    let markdown =
        convert_bytes_to_markdown(&input, None, false, None).expect("conversion should succeed");
    assert_eq!(markdown, expected);
}
