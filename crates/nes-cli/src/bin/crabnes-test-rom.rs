use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use nes_cli::test_rom::{TestOptions, TestOutcome, run_test_rom};

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<bool, Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut options = TestOptions::default();
    let mut inputs = Vec::new();

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--max-cycles" => {
                options.max_cycles = args.next().ok_or("--max-cycles needs a number")?.parse()?;
            }
            "--max-resets" => {
                options.max_resets = args.next().ok_or("--max-resets needs a number")?.parse()?;
            }
            "-h" | "--help" => {
                print_usage();
                return Ok(true);
            }
            _ if argument.starts_with('-') => {
                return Err(format!("unknown option: {argument}").into());
            }
            _ => inputs.push(PathBuf::from(argument)),
        }
    }

    if inputs.is_empty() {
        print_usage();
        return Err("provide at least one .nes test ROM or directory".into());
    }

    let mut roms = Vec::new();
    for input in inputs {
        collect_roms(&input, &mut roms)?;
    }
    roms.sort();
    roms.dedup();
    if roms.is_empty() {
        return Err("no .nes files were found".into());
    }

    let mut all_passed = true;
    for path in roms {
        let bytes = fs::read(&path)?;
        match run_test_rom(&bytes, options) {
            Ok(report) => {
                let detail = if report.message.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", report.message.replace(['\r', '\n'], " "))
                };
                match report.outcome {
                    TestOutcome::Passed => println!(
                        "PASS {} ({} cycles, {} instructions, {} resets){detail}",
                        path.display(),
                        report.cpu_cycles,
                        report.instructions,
                        report.resets
                    ),
                    TestOutcome::Failed(code) => {
                        all_passed = false;
                        println!(
                            "FAIL {} code ${code:02X} ({} cycles, {} instructions, {} resets){detail}",
                            path.display(),
                            report.cpu_cycles,
                            report.instructions,
                            report.resets
                        );
                    }
                    TestOutcome::TimedOut => {
                        all_passed = false;
                        println!(
                            "TIMEOUT {} after {} cycles and {} instructions{detail}",
                            path.display(),
                            report.cpu_cycles,
                            report.instructions
                        );
                    }
                }
            }
            Err(error) => {
                all_passed = false;
                println!("ERROR {} — {error}", path.display());
            }
        }
    }
    Ok(all_passed)
}

fn collect_roms(path: &Path, output: &mut Vec<PathBuf>) -> Result<(), std::io::Error> {
    if path.is_dir() {
        for entry in fs::read_dir(path)? {
            collect_roms(&entry?.path(), output)?;
        }
    } else if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("nes"))
    {
        output.push(path.to_path_buf());
    }
    Ok(())
}

fn print_usage() {
    println!("Usage: crabnes-test-rom [--max-cycles N] [--max-resets N] <test.nes|directory>...");
    println!("Runs ROMs that implement the standard $6000 blargg test protocol.");
}
