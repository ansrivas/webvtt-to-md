use std::cmp::Ordering;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{Encoding, UTF_8};
use html_escape::decode_html_entities;
use once_cell::sync::Lazy;
use regex::Regex;

pub const ENCODING_ERROR_MARKER: &str = "[ENC?]";

const GENERATION_SOURCE_SAMPLE_BLOCKS: usize = 50;
const RENDER_CHUNK_BLOCKS: usize = 500;
const WEBVTT_HEADER_PREFIX: &str = "WEBVTT";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueBlock {
    pub identifier: Option<String>,
    pub start: String,
    pub end: String,
    pub text: String,
    pub voice: Option<String>,
    pub raw_lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteBlock {
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleBlock {
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebVttBlock {
    Cue(CueBlock),
    Note(NoteBlock),
    Style(StyleBlock),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebVttDocument {
    pub blocks: Vec<WebVttBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationSourceMatch {
    pub generation_source: &'static str,
    pub confidence: i32,
    pub importance: i32,
    pub module_name: &'static str,
}

#[derive(Debug, Clone)]
pub enum ConverterError {
    Parse(String),
    UnsupportedGenerationSource(String),
    Decode(String),
}

impl std::fmt::Display for ConverterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(msg) => write!(f, "{msg}"),
            Self::UnsupportedGenerationSource(src) => {
                write!(f, "Unsupported generation source: {src}")
            }
            Self::Decode(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ConverterError {}

static VOICE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?s)^<v(?:\.[^>\s]+)?\s+([^>]+)>(.*)</v>$").expect("valid regex"));
static TIMING_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?P<start>\S+)\s+-->\s+(?P<end>\S+)(?:\s+.*)?$").expect("valid regex")
});
static TEAMS_IDENTIFIER_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(?P<group>.+?)(?:-|\.)(?P<segment>\d+)$").expect("valid regex"));
static TIMESTAMP_TAG_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<\d{2}:\d{2}:\d{2}\.\d{3}>").expect("valid regex"));
static BOLD_PAIR_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<b(?:\.[^>\s]+)?>(.*?)</b>").expect("valid regex"));
static ITALIC_PAIR_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<i(?:\.[^>\s]+)?>(.*?)</i>").expect("valid regex"));
static UNDERLINE_PAIR_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<u(?:\.[^>\s]+)?>(.*?)</u>").expect("valid regex"));
static CLASS_PAIR_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<c(?:\.[^>\s>]+)?>(.*?)</c>").expect("valid regex"));
static LANG_PAIR_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?is)<lang(?:\.[^>\s]+)?\s+[^>]+>(.*?)</lang>").expect("valid regex")
});
static RUBY_PAIR_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?is)<ruby(?:\.[^>\s]+)?>(.*?)</ruby>").expect("valid regex"));

static HEADER_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\*\*\((?P<timestamp>[^)]+)\)(?: (?P<speaker>.+))?\*\*$").expect("valid regex")
});
static LIST_LINE_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*(?:[-+*]|\d+[.)])\s+").expect("valid regex"));

const REFLOW_GENERATION_SOURCES: &[&str] = &[
    "ms_teams_scheduled",
    "ms_teams_event",
    "zoom_scheduled",
    "zoom_event",
    "meeting_novoicetag",
];

fn decode_html(text: &str) -> String {
    decode_html_entities(text).into_owned()
}

fn extract_voice_span(text: &str) -> (Option<String>, String) {
    if let Some(caps) = VOICE_PATTERN.captures(text) {
        let speaker = decode_html(caps.get(1).map_or("", |m| m.as_str()));
        let content = decode_html(caps.get(2).map_or("", |m| m.as_str()));
        return (Some(speaker), content);
    }
    (None, decode_html(text))
}

fn is_webvtt_header_block(lines: &[String], is_first_block: bool) -> bool {
    if !is_first_block || lines.is_empty() {
        return false;
    }
    let first = lines[0].trim_start_matches('\u{feff}');
    first == WEBVTT_HEADER_PREFIX || first.starts_with("WEBVTT ")
}

fn parse_note_block(lines: &[String]) -> NoteBlock {
    let first_line = &lines[0];
    let remainder = first_line[4..].trim_start();
    let mut note_lines = Vec::with_capacity(lines.len());
    if !remainder.is_empty() {
        note_lines.push(remainder.to_owned());
    }
    note_lines.extend(lines.iter().skip(1).cloned());
    NoteBlock { lines: note_lines }
}

fn parse_style_block(lines: &[String]) -> StyleBlock {
    StyleBlock {
        lines: lines.iter().skip(1).cloned().collect(),
    }
}

fn parse_cue_block(lines: &[String]) -> Result<CueBlock, ConverterError> {
    if lines.len() < 2 {
        return Err(ConverterError::Parse(
            "Each cue block must contain at least timing and text.".to_owned(),
        ));
    }

    let (identifier, timing_line, text_lines): (Option<String>, &str, &[String]) =
        if lines[0].contains("-->") {
            (None, lines[0].as_str(), &lines[1..])
        } else {
            if lines.len() < 3 {
                return Err(ConverterError::Parse(
                    "Cue blocks with identifier must contain timing and text.".to_owned(),
                ));
            }
            (Some(lines[0].clone()), lines[1].as_str(), &lines[2..])
        };

    let Some(caps) = TIMING_PATTERN.captures(timing_line) else {
        return Err(ConverterError::Parse(format!(
            "Invalid timing line: {timing_line:?}"
        )));
    };

    let joined = text_lines.join("\n");
    let (voice, text) = extract_voice_span(&joined);

    Ok(CueBlock {
        identifier,
        start: caps.name("start").map_or("", |m| m.as_str()).to_owned(),
        end: caps.name("end").map_or("", |m| m.as_str()).to_owned(),
        text,
        voice,
        raw_lines: text_lines.to_vec(),
    })
}

pub fn parse_webvtt(transcript: &str) -> Result<WebVttDocument, ConverterError> {
    let mut blocks = Vec::new();
    let mut current = Vec::<String>::new();
    let mut parsed_blocks = Vec::<Vec<String>>::new();

    for line in transcript.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                parsed_blocks.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(line.to_owned());
    }
    if !current.is_empty() {
        parsed_blocks.push(current);
    }

    for (idx, lines) in parsed_blocks.into_iter().enumerate() {
        if is_webvtt_header_block(&lines, idx == 0) {
            continue;
        }
        if lines[0].starts_with("NOTE") {
            blocks.push(WebVttBlock::Note(parse_note_block(&lines)));
            continue;
        }
        if lines[0] == "STYLE" {
            blocks.push(WebVttBlock::Style(parse_style_block(&lines)));
            continue;
        }
        if lines[0].contains("-->") && lines.len() == 1 {
            continue;
        }
        if !lines[0].contains("-->") && lines.len() == 2 && lines[1].contains("-->") {
            continue;
        }

        let cue = parse_cue_block(&lines)?;
        if cue.text.trim().is_empty() {
            continue;
        }
        blocks.push(WebVttBlock::Cue(cue));
    }

    Ok(WebVttDocument { blocks })
}

fn parse_ms_teams_scheduled_identifier(identifier: Option<&str>) -> Option<(String, usize)> {
    let identifier = identifier?;
    if !identifier.contains('/') {
        return None;
    }
    let key = identifier.rsplit('/').next()?;
    let caps = TEAMS_IDENTIFIER_PATTERN.captures(key)?;
    let group = caps.name("group")?.as_str().to_owned();
    let segment = caps.name("segment")?.as_str().parse::<usize>().ok()?;
    Some((group, segment))
}

fn format_header(start: &str, speaker: Option<&str>) -> String {
    let normalized_start = start.split('.').next().unwrap_or(start);
    match speaker {
        Some(speaker) if !speaker.is_empty() => format!("**({normalized_start}) {speaker}**"),
        _ => format!("**({normalized_start})**"),
    }
}

fn render_note_block(lines: &[String]) -> String {
    lines
        .iter()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_inline_tags(text: &str) -> String {
    let mut rendered = TIMESTAMP_TAG_PATTERN
        .replace_all(&decode_html(text), "")
        .into_owned();

    loop {
        let previous = rendered.clone();
        rendered = BOLD_PAIR_PATTERN
            .replace_all(&rendered, "**${1}**")
            .into_owned();
        rendered = ITALIC_PAIR_PATTERN
            .replace_all(&rendered, "*${1}*")
            .into_owned();
        rendered = UNDERLINE_PAIR_PATTERN
            .replace_all(&rendered, "_${1}_")
            .into_owned();
        rendered = CLASS_PAIR_PATTERN
            .replace_all(&rendered, "${1}")
            .into_owned();
        rendered = LANG_PAIR_PATTERN
            .replace_all(&rendered, "${1}")
            .into_owned();

        let current = rendered.clone();
        rendered = RUBY_PAIR_PATTERN
            .replace_all(&current, |caps: &regex::Captures<'_>| {
                let whole = caps.get(0).expect("whole match");
                let ruby_text = caps.get(1).map_or("", |m| m.as_str());

                let mut prefix = "";
                let mut suffix = "";

                if whole.start() > 0 {
                    let before = current[..whole.start()].chars().last();
                    if before.is_some_and(|ch| !ch.is_whitespace()) {
                        prefix = " ";
                    }
                }

                if whole.end() < current.len() {
                    let after = current[whole.end()..].chars().next();
                    if after.is_some_and(|ch| !ch.is_whitespace()) {
                        suffix = " ";
                    }
                }

                format!("{prefix}({ruby_text}){suffix}")
            })
            .into_owned();

        if rendered == previous {
            break;
        }
    }

    rendered
}

fn cue_blocks(document: &WebVttDocument) -> Vec<&CueBlock> {
    document
        .blocks
        .iter()
        .filter_map(|block| match block {
            WebVttBlock::Cue(cue) => Some(cue),
            _ => None,
        })
        .collect()
}

fn numeric_identifiers_from_document(document: &WebVttDocument) -> Vec<usize> {
    cue_blocks(document)
        .into_iter()
        .filter_map(|cue| cue.identifier.as_deref())
        .filter(|id| id.chars().all(|ch| ch.is_ascii_digit()))
        .filter_map(|id| id.parse::<usize>().ok())
        .collect()
}

fn is_strict_zoom_numeric_sequence(document: &WebVttDocument) -> bool {
    let identifiers = numeric_identifiers_from_document(document);
    if identifiers.is_empty() || identifiers[0] != 1 {
        return false;
    }
    identifiers.windows(2).all(|w| w[1] > w[0])
}

fn all_cues_single_line(document: &WebVttDocument) -> bool {
    cue_blocks(document)
        .into_iter()
        .all(|cue| !cue.text.contains('\n'))
}

fn split_textual_speaker(text: &str, use_last_colon: bool) -> Option<(String, String)> {
    let delimiter_index = if use_last_colon {
        text.rfind(':')?
    } else {
        text.find(':')?
    };

    if delimiter_index == 0 {
        return None;
    }

    let speaker = text[..delimiter_index].trim();
    let spoken_text = text[delimiter_index + 1..].trim();

    if speaker.is_empty() || spoken_text.is_empty() {
        return None;
    }

    Some((speaker.to_owned(), spoken_text.to_owned()))
}

fn is_empty_textual_speaker(text: &str) -> bool {
    let Some(delimiter_index) = text.find(':') else {
        return false;
    };
    let speaker = text[..delimiter_index].trim();
    let spoken_text = text[delimiter_index + 1..].trim();
    !speaker.is_empty() && spoken_text.is_empty()
}

fn has_textual_speaker(text: &str) -> bool {
    split_textual_speaker(text, false).is_some()
}

fn count_multi_colon_cues(document: &WebVttDocument) -> usize {
    cue_blocks(document)
        .into_iter()
        .filter(|cue| cue.text.matches(':').count() > 1)
        .count()
}

fn use_last_colon_split(document: &WebVttDocument) -> bool {
    count_multi_colon_cues(document) >= 3
}

fn match_confidence_ms_teams_scheduled(document: &WebVttDocument) -> i32 {
    let mut matched_cues = 0;
    let mut header_cues = 0;
    let mut has_voice = false;

    for cue in cue_blocks(document) {
        if cue.voice.is_some() {
            has_voice = true;
        }
        let parsed = parse_ms_teams_scheduled_identifier(cue.identifier.as_deref());
        let Some((_, segment)) = parsed else {
            continue;
        };
        matched_cues += 1;
        if segment == 0 || segment == 1 {
            header_cues += 1;
        }
    }

    if !has_voice || matched_cues == 0 {
        return 0;
    }
    if header_cues > 0 {
        return 10;
    }
    if matched_cues >= 2 {
        return 8;
    }
    6
}

fn render_ms_teams_like(document: &WebVttDocument) -> String {
    let mut groups: std::collections::HashMap<String, Vec<(usize, CueBlock)>> =
        std::collections::HashMap::new();
    let mut ordered_keys: Vec<String> = Vec::new();
    let mut leading_blocks = Vec::new();

    for block in &document.blocks {
        match block {
            WebVttBlock::Note(note) => leading_blocks.push(render_note_block(&note.lines)),
            WebVttBlock::Style(_) => {}
            WebVttBlock::Cue(cue) => {
                let parsed_identifier =
                    parse_ms_teams_scheduled_identifier(cue.identifier.as_deref());
                if let Some((group_key, segment_index)) = parsed_identifier {
                    if !groups.contains_key(&group_key) {
                        ordered_keys.push(group_key.clone());
                    }
                    groups
                        .entry(group_key)
                        .or_default()
                        .push((segment_index, cue.clone()));
                } else {
                    leading_blocks.push(format!(
                        "{}\n{}",
                        format_header(&cue.start, cue.voice.as_deref()),
                        render_inline_tags(&cue.text)
                    ));
                }
            }
        }
    }

    let mut markdown_blocks = leading_blocks;
    for group_key in ordered_keys {
        let Some(cues) = groups.get(&group_key) else {
            continue;
        };
        let header_cue = cues
            .iter()
            .find(|(segment, _)| *segment == 0)
            .or_else(|| cues.iter().find(|(segment, _)| *segment == 1))
            .unwrap_or(&cues[0]);

        let header_block = &header_cue.1;
        let body = cues
            .iter()
            .map(|(_, cue)| render_inline_tags(&cue.text))
            .collect::<Vec<_>>()
            .join("\n");

        markdown_blocks.push(format!(
            "{}\n{}",
            format_header(&header_block.start, header_block.voice.as_deref()),
            body
        ));
    }

    format!("{}\n", markdown_blocks.join("\n\n"))
}

fn match_confidence_ms_teams_event(document: &WebVttDocument) -> i32 {
    let mut matched_cues = 0;
    let mut has_voice = false;

    for cue in cue_blocks(document) {
        if cue.voice.is_some() {
            has_voice = true;
        }
        if parse_ms_teams_scheduled_identifier(cue.identifier.as_deref()).is_some() {
            matched_cues += 1;
        }
    }

    if has_voice {
        return 0;
    }
    if matched_cues >= 2 {
        return 8;
    }
    if matched_cues == 1 {
        return 6;
    }
    0
}

fn match_confidence_zoom_scheduled(document: &WebVttDocument) -> i32 {
    if !is_strict_zoom_numeric_sequence(document) {
        return 0;
    }
    if !all_cues_single_line(document) {
        return 0;
    }

    let cues = cue_blocks(document);
    if cues.len() < 2 {
        return 0;
    }
    if cues
        .iter()
        .all(|cue| split_textual_speaker(&cue.text, false).is_some())
    {
        return 7;
    }
    0
}

fn render_zoom_scheduled(document: &WebVttDocument) -> String {
    let use_last_colon = use_last_colon_split(document);
    let mut markdown_blocks = Vec::new();

    for block in &document.blocks {
        match block {
            WebVttBlock::Note(note) => markdown_blocks.push(render_note_block(&note.lines)),
            WebVttBlock::Style(_) => {}
            WebVttBlock::Cue(cue) => {
                if is_empty_textual_speaker(&cue.text) {
                    continue;
                }
                if let Some((speaker, spoken_text)) =
                    split_textual_speaker(&cue.text, use_last_colon)
                {
                    markdown_blocks.push(format!(
                        "{}\n{}",
                        format_header(&cue.start, Some(&speaker)),
                        render_inline_tags(&spoken_text)
                    ));
                } else {
                    markdown_blocks.push(format!(
                        "{}\n{}",
                        format_header(&cue.start, cue.voice.as_deref()),
                        render_inline_tags(&cue.text)
                    ));
                }
            }
        }
    }

    format!("{}\n", markdown_blocks.join("\n\n"))
}

fn match_confidence_zoom_event(document: &WebVttDocument) -> i32 {
    let cues = cue_blocks(document);
    if cues.is_empty() {
        return 0;
    }
    if !is_strict_zoom_numeric_sequence(document) {
        return 0;
    }
    if !all_cues_single_line(document) {
        return 0;
    }
    if cues.len() == 1 {
        return 6;
    }
    if cues.iter().any(|cue| has_textual_speaker(&cue.text)) {
        return 0;
    }
    6
}

fn render_zoom_event(document: &WebVttDocument) -> String {
    let mut markdown_blocks = Vec::new();
    for block in &document.blocks {
        match block {
            WebVttBlock::Note(note) => markdown_blocks.push(render_note_block(&note.lines)),
            WebVttBlock::Style(_) => {}
            WebVttBlock::Cue(cue) => markdown_blocks.push(format!(
                "{}\n{}",
                format_header(&cue.start, cue.voice.as_deref()),
                render_inline_tags(&cue.text)
            )),
        }
    }
    format!("{}\n", markdown_blocks.join("\n\n"))
}

fn match_confidence_meeting_novoicetag(document: &WebVttDocument) -> i32 {
    let cues = cue_blocks(document);
    if cues.is_empty() {
        return 0;
    }
    if cues.iter().any(|cue| cue.voice.is_some()) {
        return 0;
    }
    if cues.iter().any(|cue| {
        cue.identifier
            .as_deref()
            .is_some_and(|id| id.chars().all(|ch| ch.is_ascii_digit()))
    }) {
        return 0;
    }
    if cues.iter().all(|cue| has_textual_speaker(&cue.text)) {
        return 5;
    }
    0
}

fn render_meeting_novoicetag(document: &WebVttDocument) -> String {
    let mut markdown_blocks = Vec::new();
    for block in &document.blocks {
        match block {
            WebVttBlock::Note(note) => markdown_blocks.push(render_note_block(&note.lines)),
            WebVttBlock::Style(_) => {}
            WebVttBlock::Cue(cue) => {
                if is_empty_textual_speaker(&cue.text) {
                    continue;
                }
                if let Some((speaker, spoken_text)) = split_textual_speaker(&cue.text, false) {
                    markdown_blocks.push(format!(
                        "{}\n{}",
                        format_header(&cue.start, Some(&speaker)),
                        render_inline_tags(&spoken_text)
                    ));
                } else {
                    markdown_blocks.push(format!(
                        "{}\n{}",
                        format_header(&cue.start, cue.voice.as_deref()),
                        render_inline_tags(&cue.text)
                    ));
                }
            }
        }
    }
    format!("{}\n", markdown_blocks.join("\n\n"))
}

fn match_confidence_generic_webvtt(_: &WebVttDocument) -> i32 {
    1
}

fn render_generic_webvtt(document: &WebVttDocument) -> String {
    let mut markdown_blocks = Vec::new();
    for block in &document.blocks {
        match block {
            WebVttBlock::Note(note) => markdown_blocks.push(render_note_block(&note.lines)),
            WebVttBlock::Style(_) => {}
            WebVttBlock::Cue(cue) => markdown_blocks.push(format!(
                "{}\n{}",
                format_header(&cue.start, cue.voice.as_deref()),
                render_inline_tags(&cue.text)
            )),
        }
    }
    format!("{}\n", markdown_blocks.join("\n\n"))
}

struct RuleSet {
    generation_source: &'static str,
    importance: i32,
    module_name: &'static str,
    match_confidence: fn(&WebVttDocument) -> i32,
    render: fn(&WebVttDocument) -> String,
}

const RULESETS: &[RuleSet] = &[
    RuleSet {
        generation_source: "generic_webvtt",
        importance: 100,
        module_name: "gen_src_generic_webvtt",
        match_confidence: match_confidence_generic_webvtt,
        render: render_generic_webvtt,
    },
    RuleSet {
        generation_source: "meeting_novoicetag",
        importance: 400,
        module_name: "gen_src_meeting_novoicetag",
        match_confidence: match_confidence_meeting_novoicetag,
        render: render_meeting_novoicetag,
    },
    RuleSet {
        generation_source: "ms_teams_event",
        importance: 650,
        module_name: "gen_src_ms_teams_event",
        match_confidence: match_confidence_ms_teams_event,
        render: render_ms_teams_like,
    },
    RuleSet {
        generation_source: "ms_teams_scheduled",
        importance: 700,
        module_name: "gen_src_ms_teams_scheduled",
        match_confidence: match_confidence_ms_teams_scheduled,
        render: render_ms_teams_like,
    },
    RuleSet {
        generation_source: "zoom_event",
        importance: 500,
        module_name: "gen_src_zoom_event",
        match_confidence: match_confidence_zoom_event,
        render: render_zoom_event,
    },
    RuleSet {
        generation_source: "zoom_scheduled",
        importance: 600,
        module_name: "gen_src_zoom_scheduled",
        match_confidence: match_confidence_zoom_scheduled,
        render: render_zoom_scheduled,
    },
];

pub fn score_generation_sources(document: &WebVttDocument) -> Vec<GenerationSourceMatch> {
    let sampled = WebVttDocument {
        blocks: document
            .blocks
            .iter()
            .take(GENERATION_SOURCE_SAMPLE_BLOCKS)
            .cloned()
            .collect(),
    };

    RULESETS
        .iter()
        .map(|ruleset| {
            let mut confidence = (ruleset.match_confidence)(&sampled);
            confidence = confidence.clamp(0, 10);
            GenerationSourceMatch {
                generation_source: ruleset.generation_source,
                confidence,
                importance: ruleset.importance,
                module_name: ruleset.module_name,
            }
        })
        .collect()
}

fn compare_matches(a: &GenerationSourceMatch, b: &GenerationSourceMatch) -> Ordering {
    (a.confidence, a.importance, a.module_name).cmp(&(b.confidence, b.importance, b.module_name))
}

pub fn detect_generation_source(document: &WebVttDocument) -> Result<&'static str, ConverterError> {
    let scores = score_generation_sources(document);
    let Some(winner) = scores.iter().max_by(|a, b| compare_matches(a, b)) else {
        return Err(ConverterError::Parse(
            "No generation source modules registered.".to_owned(),
        ));
    };
    Ok(winner.generation_source)
}

fn get_ruleset(
    generation_source: Option<&str>,
    document: Option<&WebVttDocument>,
) -> Result<&'static RuleSet, ConverterError> {
    if let Some(source) = generation_source {
        return RULESETS
            .iter()
            .find(|ruleset| ruleset.generation_source == source)
            .ok_or_else(|| ConverterError::UnsupportedGenerationSource(source.to_owned()));
    }

    let document = document.ok_or_else(|| {
        ConverterError::Parse(
            "document is required when generation_source is not provided.".to_owned(),
        )
    })?;
    let winner = detect_generation_source(document)?;

    RULESETS
        .iter()
        .find(|ruleset| ruleset.generation_source == winner)
        .ok_or_else(|| ConverterError::Parse("No generation source modules registered.".to_owned()))
}

#[derive(Debug, Clone)]
struct MarkdownBlock {
    raw_text: String,
    header: Option<String>,
    speaker: Option<String>,
    body: String,
}

fn parse_markdown_block(raw_block: &str) -> MarkdownBlock {
    let lines: Vec<&str> = raw_block.lines().collect();
    if lines.is_empty() {
        return MarkdownBlock {
            raw_text: raw_block.to_owned(),
            header: None,
            speaker: None,
            body: String::new(),
        };
    }

    if let Some(caps) = HEADER_PATTERN.captures(lines[0]) {
        return MarkdownBlock {
            raw_text: raw_block.to_owned(),
            header: Some(lines[0].to_owned()),
            speaker: caps.name("speaker").map(|m| m.as_str().to_owned()),
            body: lines[1..].join("\n"),
        };
    }

    MarkdownBlock {
        raw_text: raw_block.to_owned(),
        header: None,
        speaker: None,
        body: lines[1..].join("\n"),
    }
}

fn contains_list_like_line(body: &str) -> bool {
    body.lines().any(|line| LIST_LINE_PATTERN.is_match(line))
}

fn can_stitch(previous: &MarkdownBlock, current: &MarkdownBlock) -> bool {
    if previous.header.is_none() || current.header.is_none() {
        return false;
    }
    let Some(prev_speaker) = &previous.speaker else {
        return false;
    };
    let Some(curr_speaker) = &current.speaker else {
        return false;
    };
    if prev_speaker != curr_speaker {
        return false;
    }
    if contains_list_like_line(&previous.body) || contains_list_like_line(&current.body) {
        return false;
    }
    true
}

fn render_markdown_block(block: &MarkdownBlock) -> String {
    if block.header.is_none() {
        return block.raw_text.clone();
    }
    if block.body.is_empty() {
        return block.header.clone().unwrap_or_default();
    }
    format!(
        "{}\n{}",
        block.header.clone().unwrap_or_default(),
        block.body
    )
}

fn should_reflow_block(block: &MarkdownBlock, generation_source: Option<&str>) -> bool {
    if !generation_source.is_some_and(|source| REFLOW_GENERATION_SOURCES.contains(&source)) {
        return false;
    }
    if block.header.is_none() || block.body.is_empty() {
        return false;
    }
    if contains_list_like_line(&block.body) {
        return false;
    }
    if block.body.lines().any(|line| line.trim().is_empty()) {
        return false;
    }
    block.body.contains('\n')
}

fn reflow_block(block: &MarkdownBlock) -> MarkdownBlock {
    let normalized_body = block
        .body
        .lines()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join(" ");

    MarkdownBlock {
        raw_text: String::new(),
        header: block.header.clone(),
        speaker: block.speaker.clone(),
        body: normalized_body,
    }
}

pub fn postprocess_markdown(markdown: &str, generation_source: Option<&str>) -> String {
    if markdown.is_empty() {
        return String::new();
    }

    let content = markdown.strip_suffix('\n').unwrap_or(markdown);
    if content.is_empty() {
        return "\n".to_owned();
    }

    let parsed_blocks: Vec<MarkdownBlock> =
        content.split("\n\n").map(parse_markdown_block).collect();
    let mut merged_blocks: Vec<MarkdownBlock> = Vec::new();

    for block in parsed_blocks {
        if merged_blocks.is_empty() {
            merged_blocks.push(block);
            continue;
        }

        let previous = merged_blocks.last().cloned().expect("last exists");
        if !can_stitch(&previous, &block) {
            merged_blocks.push(block);
            continue;
        }

        let merged_body = if previous.body.is_empty() {
            block.body.clone()
        } else if block.body.is_empty() {
            previous.body.clone()
        } else {
            format!("{}\n{}", previous.body, block.body)
        };

        if let Some(last) = merged_blocks.last_mut() {
            *last = MarkdownBlock {
                raw_text: String::new(),
                header: previous.header,
                speaker: previous.speaker,
                body: merged_body,
            };
        }
    }

    let reflowed = merged_blocks
        .into_iter()
        .map(|block| {
            if should_reflow_block(&block, generation_source) {
                reflow_block(&block)
            } else {
                block
            }
        })
        .collect::<Vec<_>>();

    format!(
        "{}\n",
        reflowed
            .iter()
            .map(render_markdown_block)
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

pub fn render_markdown(
    document: &WebVttDocument,
    generation_source: Option<&str>,
) -> Result<String, ConverterError> {
    let ruleset = get_ruleset(generation_source, Some(document))?;
    if document.blocks.len() <= RENDER_CHUNK_BLOCKS {
        return Ok(postprocess_markdown(
            &(ruleset.render)(document),
            Some(ruleset.generation_source),
        ));
    }

    let mut rendered_chunks = Vec::<String>::new();

    for start in (0..document.blocks.len()).step_by(RENDER_CHUNK_BLOCKS) {
        let chunk_document = WebVttDocument {
            blocks: document
                .blocks
                .iter()
                .skip(start)
                .take(RENDER_CHUNK_BLOCKS)
                .cloned()
                .collect(),
        };

        let chunk_markdown = postprocess_markdown(
            &(ruleset.render)(&chunk_document),
            Some(ruleset.generation_source),
        )
        .trim_end_matches('\n')
        .to_owned();

        if !chunk_markdown.is_empty() {
            rendered_chunks.push(chunk_markdown);
        }
    }

    if rendered_chunks.is_empty() {
        return Ok("\n".to_owned());
    }

    Ok(format!("{}\n", rendered_chunks.join("\n\n")))
}

pub fn transcript_to_markdown(transcript: &str) -> Result<String, ConverterError> {
    let document = parse_webvtt(transcript)?;
    render_markdown(&document, None)
}

fn trim_bom_prefix(text: String) -> String {
    text.trim_start_matches('\u{feff}').to_owned()
}

fn looks_like_utf16(data: &[u8]) -> Option<&'static str> {
    let sample = &data[..data.len().min(128)];
    if sample.len() < 4 || sample.len() % 2 != 0 {
        return None;
    }

    let mut even_nuls = 0usize;
    let mut odd_nuls = 0usize;
    for idx in (0..sample.len()).step_by(2) {
        if sample[idx] == 0 {
            even_nuls += 1;
        }
        if sample[idx + 1] == 0 {
            odd_nuls += 1;
        }
    }

    let half_length = sample.len() / 2;
    let threshold = std::cmp::max(4, half_length / 2);

    if odd_nuls >= threshold && even_nuls == 0 {
        return Some("utf-16le");
    }
    if even_nuls >= threshold && odd_nuls == 0 {
        return Some("utf-16be");
    }

    None
}

fn decode_utf16_strict(data: &[u8], little_endian: bool) -> Result<String, ConverterError> {
    if data.len() % 2 != 0 {
        return Err(ConverterError::Decode(
            "Invalid UTF-16 byte length.".to_owned(),
        ));
    }

    let mut units = Vec::with_capacity(data.len() / 2);
    for chunk in data.chunks_exact(2) {
        let unit = if little_endian {
            u16::from_le_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], chunk[1]])
        };
        units.push(unit);
    }

    String::from_utf16(&units)
        .map_err(|_| ConverterError::Decode("Invalid UTF-16 sequence.".to_owned()))
}

fn decode_utf32_strict(data: &[u8], little_endian: bool) -> Result<String, ConverterError> {
    if data.len() % 4 != 0 {
        return Err(ConverterError::Decode(
            "Invalid UTF-32 byte length.".to_owned(),
        ));
    }

    let mut out = String::with_capacity(data.len() / 4);
    for chunk in data.chunks_exact(4) {
        let value = if little_endian {
            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        } else {
            u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        };

        let Some(ch) = char::from_u32(value) else {
            return Err(ConverterError::Decode(
                "Invalid UTF-32 code point.".to_owned(),
            ));
        };
        out.push(ch);
    }

    Ok(out)
}

fn decode_with_encoding_strict(
    data: &[u8],
    encoding: &'static Encoding,
) -> Result<String, ConverterError> {
    let (decoded, had_errors) = encoding.decode_without_bom_handling(data);
    if had_errors {
        return Err(ConverterError::Decode(format!(
            "Could not decode transcript bytes with encoding {}.",
            encoding.name()
        )));
    }
    Ok(decoded.into_owned())
}

fn decode_with_encoding_marker(data: &[u8], encoding: &'static Encoding) -> String {
    let (decoded, _had_errors) = encoding.decode_without_bom_handling(data);
    decoded.replace('\u{fffd}', ENCODING_ERROR_MARKER)
}

fn decode_with_charset_strict(data: &[u8], charset: &str) -> Result<String, ConverterError> {
    let lower = charset.to_ascii_lowercase();
    match lower.as_str() {
        "utf-16" => {
            if data.starts_with(&[0xff, 0xfe]) {
                return decode_utf16_strict(&data[2..], true);
            }
            if data.starts_with(&[0xfe, 0xff]) {
                return decode_utf16_strict(&data[2..], false);
            }
            decode_utf16_strict(data, true)
        }
        "utf-16le" => decode_utf16_strict(data, true),
        "utf-16be" => decode_utf16_strict(data, false),
        "utf-32" => {
            if data.starts_with(&[0xff, 0xfe, 0x00, 0x00]) {
                return decode_utf32_strict(&data[4..], true);
            }
            if data.starts_with(&[0x00, 0x00, 0xfe, 0xff]) {
                return decode_utf32_strict(&data[4..], false);
            }
            decode_utf32_strict(data, true)
        }
        "utf-32le" => decode_utf32_strict(data, true),
        "utf-32be" => decode_utf32_strict(data, false),
        _ => {
            let Some(encoding) = Encoding::for_label(charset.as_bytes()) else {
                return Err(ConverterError::Decode(format!(
                    "Unknown charset: {charset}"
                )));
            };
            decode_with_encoding_strict(data, encoding)
        }
    }
}

fn decode_with_charset_marker(data: &[u8], charset: &str) -> String {
    let lower = charset.to_ascii_lowercase();
    match lower.as_str() {
        "utf-16" => {
            if data.starts_with(&[0xff, 0xfe]) {
                return decode_utf16_strict(&data[2..], true)
                    .unwrap_or_else(|_| decode_with_encoding_marker(&data[2..], UTF_8));
            }
            if data.starts_with(&[0xfe, 0xff]) {
                return decode_utf16_strict(&data[2..], false)
                    .unwrap_or_else(|_| decode_with_encoding_marker(&data[2..], UTF_8));
            }
            decode_utf16_strict(data, true)
                .unwrap_or_else(|_| decode_with_encoding_marker(data, UTF_8))
        }
        "utf-16le" => decode_utf16_strict(data, true)
            .unwrap_or_else(|_| decode_with_encoding_marker(data, UTF_8)),
        "utf-16be" => decode_utf16_strict(data, false)
            .unwrap_or_else(|_| decode_with_encoding_marker(data, UTF_8)),
        "utf-32" | "utf-32le" | "utf-32be" => decode_with_encoding_marker(data, UTF_8),
        _ => {
            let Some(encoding) = Encoding::for_label(charset.as_bytes()) else {
                return decode_with_encoding_marker(data, UTF_8);
            };
            decode_with_encoding_marker(data, encoding)
        }
    }
}

fn strict_decode_transcript_bytes(
    data: &[u8],
    charset: Option<&str>,
) -> Result<String, ConverterError> {
    if let Some(charset) = charset {
        return decode_with_charset_strict(data, charset).map(trim_bom_prefix);
    }

    if let Some(stripped) = data.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8(stripped.to_vec())
            .map(trim_bom_prefix)
            .map_err(|_| {
                ConverterError::Decode(
                    "Could not decode transcript bytes with UTF-8 BOM.".to_owned(),
                )
            });
    }

    if let Some(stripped) = data.strip_prefix(&[0xff, 0xfe, 0x00, 0x00]) {
        return decode_utf32_strict(stripped, true).map(trim_bom_prefix);
    }
    if let Some(stripped) = data.strip_prefix(&[0x00, 0x00, 0xfe, 0xff]) {
        return decode_utf32_strict(stripped, false).map(trim_bom_prefix);
    }
    if let Some(stripped) = data.strip_prefix(&[0xff, 0xfe]) {
        return decode_utf16_strict(stripped, true).map(trim_bom_prefix);
    }
    if let Some(stripped) = data.strip_prefix(&[0xfe, 0xff]) {
        return decode_utf16_strict(stripped, false).map(trim_bom_prefix);
    }

    if let Some(utf16_guess) = looks_like_utf16(data) {
        let decoded = if utf16_guess == "utf-16le" {
            decode_utf16_strict(data, true)
        } else {
            decode_utf16_strict(data, false)
        };
        if let Ok(value) = decoded {
            return Ok(trim_bom_prefix(value));
        }
    }

    if let Ok(value) = std::str::from_utf8(data) {
        return Ok(value.to_owned());
    }

    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(data, true);
    let detected = detector.guess(None, Utf8Detection::Allow);
    if let Ok(value) = decode_with_encoding_strict(data, detected) {
        return Ok(trim_bom_prefix(value));
    }

    for fallback in ["cp1254", "cp1251", "cp1252"] {
        if let Some(encoding) = Encoding::for_label(fallback.as_bytes()) {
            if let Ok(value) = decode_with_encoding_strict(data, encoding) {
                return Ok(value);
            }
        }
    }

    Err(ConverterError::Decode(
        "Could not decode transcript bytes. Attempted BOM detection, utf-8, cp1254, cp1251, cp1252, and charset detection."
            .to_owned(),
    ))
}

pub fn decode_transcript_bytes(
    data: &[u8],
    charset: Option<&str>,
    charset_lazy: bool,
) -> Result<String, ConverterError> {
    if !charset_lazy {
        return strict_decode_transcript_bytes(data, charset);
    }

    if let Ok(decoded) = strict_decode_transcript_bytes(data, charset) {
        return Ok(decoded);
    }

    if let Some(charset) = charset {
        return Ok(trim_bom_prefix(decode_with_charset_marker(data, charset)));
    }

    if let Some(stripped) = data.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return Ok(trim_bom_prefix(decode_with_encoding_marker(
            stripped, UTF_8,
        )));
    }
    if let Some(stripped) = data.strip_prefix(&[0xff, 0xfe]) {
        return Ok(trim_bom_prefix(
            decode_utf16_strict(stripped, true)
                .unwrap_or_else(|_| decode_with_encoding_marker(stripped, UTF_8)),
        ));
    }
    if let Some(stripped) = data.strip_prefix(&[0xfe, 0xff]) {
        return Ok(trim_bom_prefix(
            decode_utf16_strict(stripped, false)
                .unwrap_or_else(|_| decode_with_encoding_marker(stripped, UTF_8)),
        ));
    }

    if let Some(utf16_guess) = looks_like_utf16(data) {
        let guessed = if utf16_guess == "utf-16le" {
            decode_utf16_strict(data, true)
        } else {
            decode_utf16_strict(data, false)
        };
        if let Ok(value) = guessed {
            return Ok(trim_bom_prefix(value));
        }
    }

    let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
    detector.feed(data, true);
    let detected = detector.guess(None, Utf8Detection::Allow);
    let best_effort = decode_with_encoding_marker(data, detected);
    if !best_effort.is_empty() {
        return Ok(trim_bom_prefix(best_effort));
    }

    Ok(trim_bom_prefix(decode_with_encoding_marker(data, UTF_8)))
}

pub fn convert_bytes_to_markdown(
    data: &[u8],
    charset: Option<&str>,
    charset_lazy: bool,
    generation_source: Option<&str>,
) -> Result<String, ConverterError> {
    let transcript = decode_transcript_bytes(data, charset, charset_lazy)?;
    let document = parse_webvtt(&transcript)?;
    render_markdown(&document, generation_source)
}

pub fn detect_generation_source_from_bytes(
    data: &[u8],
    charset: Option<&str>,
    charset_lazy: bool,
) -> Result<&'static str, ConverterError> {
    let transcript = decode_transcript_bytes(data, charset, charset_lazy)?;
    let document = parse_webvtt(&transcript)?;
    detect_generation_source(&document)
}

pub fn decode_with_marker(data: &[u8], charset: &str) -> String {
    trim_bom_prefix(decode_with_charset_marker(data, charset))
}

pub fn convert_text_to_markdown(
    transcript: &str,
    generation_source: Option<&str>,
) -> Result<String, ConverterError> {
    let document = parse_webvtt(transcript)?;
    render_markdown(&document, generation_source)
}
