use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, ValueEnum};
use webvtt_to_md::{
    ConverterError, convert_bytes_to_markdown, detect_generation_source_from_bytes,
};

#[derive(Debug, Clone, ValueEnum)]
enum Source {
    GenericWebvtt,
    MsTeamsScheduled,
    MsTeamsEvent,
    ZoomScheduled,
    ZoomEvent,
    MeetingNovoicetag,
}

impl Source {
    fn as_generation_source(&self) -> &'static str {
        match self {
            Self::GenericWebvtt => "generic_webvtt",
            Self::MsTeamsScheduled => "ms_teams_scheduled",
            Self::MsTeamsEvent => "ms_teams_event",
            Self::ZoomScheduled => "zoom_scheduled",
            Self::ZoomEvent => "zoom_event",
            Self::MeetingNovoicetag => "meeting_novoicetag",
        }
    }
}

#[derive(Parser, Debug)]
#[command(name = "webvtt-to-md")]
#[command(
    version,
    about = "Convert WebVTT transcripts to Markdown speaker blocks",
    long_about = "High-performance WebVTT to Markdown converter.\n\nBy default the converter auto-detects a generation source (Teams/Zoom/generic), decodes input in strict mode, and writes Markdown to stdout.",
    after_help = "Examples:\n  webvtt-to-md Blueprint_way_forward.vtt\n  webvtt-to-md Blueprint_way_forward.vtt -o Blueprint_way_forward.md\n  webvtt-to-md Blueprint_way_forward.vtt --detect-source\n  webvtt-to-md Blueprint_way_forward.vtt --source generic-webvtt -o out.md\n  webvtt-to-md input.vtt --charset utf-8 --charset-lazy -o out.md"
)]
struct Args {
    #[arg(value_name = "INPUT", help = "Path to the input .vtt file")]
    input: PathBuf,

    #[arg(
        short,
        long,
        value_name = "OUTPUT",
        help = "Write Markdown to a file instead of stdout"
    )]
    output: Option<PathBuf>,

    #[arg(
        long,
        value_name = "CHARSET",
        help = "Force input charset (for example: utf-8, utf-16le, cp1251)"
    )]
    charset: Option<String>,

    #[arg(
        long,
        default_value_t = false,
        help = "Lazy decode mode: replace undecodable bytes with [ENC?]"
    )]
    charset_lazy: bool,

    #[arg(
        long,
        value_enum,
        value_name = "SOURCE",
        help = "Force a generation source instead of auto-detection"
    )]
    source: Option<Source>,

    #[arg(
        long,
        default_value_t = false,
        help = "Only detect and print source type, then exit"
    )]
    detect_source: bool,
}

fn run() -> Result<(), ConverterError> {
    let args = Args::parse();
    let data = fs::read(&args.input).map_err(|err| {
        ConverterError::Decode(format!(
            "Failed to read input file {}: {err}",
            args.input.display()
        ))
    })?;

    if args.detect_source {
        let source =
            detect_generation_source_from_bytes(&data, args.charset.as_deref(), args.charset_lazy)?;
        println!("{source}");
        return Ok(());
    }

    let markdown = convert_bytes_to_markdown(
        &data,
        args.charset.as_deref(),
        args.charset_lazy,
        args.source.as_ref().map(Source::as_generation_source),
    )?;

    if let Some(output) = args.output {
        fs::write(&output, markdown).map_err(|err| {
            ConverterError::Decode(format!(
                "Failed to write output file {}: {err}",
                output.display()
            ))
        })?;
    } else {
        let mut stdout = io::stdout().lock();
        stdout.write_all(markdown.as_bytes()).map_err(|err| {
            ConverterError::Decode(format!("Failed to write markdown to stdout: {err}"))
        })?;
    }

    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
