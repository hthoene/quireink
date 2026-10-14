//! `quireink-fontgen` — converts fonts into QuireInk bitmap font packs.
//!
//! Devices without runtime font rendering (ESP32-C3) read `.qifp` packs
//! directly from flash or storage. Packs are rendered with the same
//! rasterizer QuireInk uses at runtime, so text looks identical.

use std::process::ExitCode;

const USAGE: &str = "\
quireink-fontgen — build QuireInk bitmap font packs

USAGE:
    quireink-fontgen <font.ttf|otf> <output.qifp> [OPTIONS]

OPTIONS:
    --sizes <px,...>      Pixel sizes to render (default: 20,24,28,32,36,40,48)
    --axis <tag=value>    Set a variation axis, e.g. --axis wght=700 (repeatable)
    -h, --help            Show this help
";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let mut positional = Vec::new();
    let mut sizes = vec![20.0, 24.0, 28.0, 32.0, 36.0, 40.0, 48.0];
    let mut axes: Vec<(String, f32)> = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "--sizes" => {
                let list = args.next().ok_or("--sizes requires a value")?;
                sizes = list
                    .split(',')
                    .map(|s| {
                        s.trim()
                            .parse::<f32>()
                            .map_err(|_| format!("invalid size `{s}`"))
                    })
                    .collect::<Result<_, _>>()?;
            }
            "--axis" => {
                let axis = args.next().ok_or("--axis requires a value")?;
                let (tag, value) = axis
                    .split_once('=')
                    .ok_or_else(|| format!("expected <tag>=<value>, got `{axis}`"))?;
                let value = value
                    .parse()
                    .map_err(|_| format!("invalid axis value `{value}`"))?;
                axes.push((tag.to_string(), value));
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option `{arg}`")),
            _ => positional.push(arg),
        }
    }
    let [input, output] = positional.as_slice() else {
        return Err(format!("expected an input and an output file\n\n{USAGE}"));
    };

    let data = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let variations: Vec<(&str, f32)> = axes.iter().map(|(t, v)| (t.as_str(), *v)).collect();
    let font =
        quireink_text::OutlineFont::new(data, &variations).map_err(|e| format!("{input}: {e}"))?;
    let chars = quireink_text::charset::chars(quireink_text::charset::LATIN);
    let pack = quireink_text::build_pack(&font, &sizes, chars);
    std::fs::write(output, &pack).map_err(|e| format!("{output}: {e}"))?;
    println!(
        "{output}: {} sizes, {:.1} KiB",
        sizes.len(),
        pack.len() as f64 / 1024.0
    );
    Ok(())
}
