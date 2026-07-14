use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use nes_cli::test_rom::{TestOptions, TestOutcome, run_test_rom};
use serde::Serialize;

const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum RecordStatus {
    Passed,
    Failed,
    TimedOut,
    Error,
}

#[derive(Debug, Serialize)]
struct AccuracyRecord {
    path: String,
    category: String,
    status: RecordStatus,
    failure_code: Option<u8>,
    message: String,
    cpu_cycles: u64,
    instructions: u64,
    resets: u32,
}

#[derive(Debug, Default, Serialize)]
struct CategorySummary {
    total: usize,
    passed: usize,
    failed: usize,
    timed_out: usize,
    errors: usize,
}

#[derive(Debug, Serialize)]
struct AccuracyReport {
    schema_version: u32,
    all_passed: bool,
    totals: CategorySummary,
    categories: BTreeMap<String, CategorySummary>,
    tests: Vec<AccuracyRecord>,
}

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
    let mut json_output = None;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--max-cycles" => {
                options.max_cycles = args.next().ok_or("--max-cycles needs a number")?.parse()?;
            }
            "--max-resets" => {
                options.max_resets = args.next().ok_or("--max-resets needs a number")?.parse()?;
            }
            "--json" => {
                json_output = Some(PathBuf::from(
                    args.next().ok_or("--json needs an output path")?,
                ));
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

    let mut records = Vec::with_capacity(roms.len());
    for path in roms {
        let bytes = fs::read(&path)?;
        let category = classify_test(&path).to_owned();
        match run_test_rom(&bytes, options) {
            Ok(report) => {
                let detail = if report.message.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", report.message.replace(['\r', '\n'], " "))
                };
                match report.outcome {
                    TestOutcome::Passed => {
                        println!(
                            "PASS [{category}] {} ({} cycles, {} instructions, {} resets){detail}",
                            path.display(),
                            report.cpu_cycles,
                            report.instructions,
                            report.resets
                        );
                        records.push(AccuracyRecord {
                            path: path.display().to_string(),
                            category,
                            status: RecordStatus::Passed,
                            failure_code: None,
                            message: report.message,
                            cpu_cycles: report.cpu_cycles,
                            instructions: report.instructions,
                            resets: report.resets,
                        });
                    }
                    TestOutcome::Failed(code) => {
                        println!(
                            "FAIL [{category}] {} code ${code:02X} ({} cycles, {} instructions, {} resets){detail}",
                            path.display(),
                            report.cpu_cycles,
                            report.instructions,
                            report.resets
                        );
                        records.push(AccuracyRecord {
                            path: path.display().to_string(),
                            category,
                            status: RecordStatus::Failed,
                            failure_code: Some(code),
                            message: report.message,
                            cpu_cycles: report.cpu_cycles,
                            instructions: report.instructions,
                            resets: report.resets,
                        });
                    }
                    TestOutcome::TimedOut => {
                        println!(
                            "TIMEOUT [{category}] {} after {} cycles and {} instructions{detail}",
                            path.display(),
                            report.cpu_cycles,
                            report.instructions
                        );
                        records.push(AccuracyRecord {
                            path: path.display().to_string(),
                            category,
                            status: RecordStatus::TimedOut,
                            failure_code: None,
                            message: report.message,
                            cpu_cycles: report.cpu_cycles,
                            instructions: report.instructions,
                            resets: report.resets,
                        });
                    }
                }
            }
            Err(error) => {
                let message = error.to_string();
                println!("ERROR [{category}] {} — {message}", path.display());
                records.push(AccuracyRecord {
                    path: path.display().to_string(),
                    category,
                    status: RecordStatus::Error,
                    failure_code: None,
                    message,
                    cpu_cycles: 0,
                    instructions: 0,
                    resets: 0,
                });
            }
        }
    }

    let report = build_accuracy_report(records);
    print_summary(&report);
    if let Some(path) = json_output {
        let json = serde_json::to_vec_pretty(&report)?;
        fs::write(&path, json)?;
        println!("JSON report: {}", path.display());
    }
    Ok(report.all_passed)
}

fn classify_test(path: &Path) -> &'static str {
    let name = path.to_string_lossy().to_ascii_lowercase();
    if ["ppu", "sprite", "vbl", "palette", "scroll"]
        .iter()
        .any(|term| name.contains(term))
    {
        "PPU"
    } else if ["apu", "dmc", "dpcm", "audio"]
        .iter()
        .any(|term| name.contains(term))
    {
        "APU"
    } else if ["mapper", "mmc", "vrc", "nrom", "uxrom", "cnrom", "fme"]
        .iter()
        .any(|term| name.contains(term))
    {
        "Mapper"
    } else if ["input", "joy", "controller", "pad", "zapper"]
        .iter()
        .any(|term| name.contains(term))
    {
        "Input"
    } else if ["dma", "timing", "clock"]
        .iter()
        .any(|term| name.contains(term))
    {
        "Timing"
    } else if ["cpu", "instr", "branch", "interrupt", "opcode", "nestest"]
        .iter()
        .any(|term| name.contains(term))
    {
        "CPU"
    } else {
        "Other"
    }
}

fn build_accuracy_report(tests: Vec<AccuracyRecord>) -> AccuracyReport {
    let mut categories = BTreeMap::<String, CategorySummary>::new();
    let mut totals = CategorySummary::default();
    for test in &tests {
        update_summary(&mut totals, test.status);
        update_summary(
            categories.entry(test.category.clone()).or_default(),
            test.status,
        );
    }
    AccuracyReport {
        schema_version: REPORT_SCHEMA_VERSION,
        all_passed: totals.total == totals.passed,
        totals,
        categories,
        tests,
    }
}

fn update_summary(summary: &mut CategorySummary, status: RecordStatus) {
    summary.total += 1;
    match status {
        RecordStatus::Passed => summary.passed += 1,
        RecordStatus::Failed => summary.failed += 1,
        RecordStatus::TimedOut => summary.timed_out += 1,
        RecordStatus::Error => summary.errors += 1,
    }
}

fn print_summary(report: &AccuracyReport) {
    println!();
    println!("Accuracy summary");
    for (category, summary) in &report.categories {
        println!(
            "  {category:<8} {:>3}/{:<3} passed  ({} failed, {} timeout, {} error)",
            summary.passed, summary.total, summary.failed, summary.timed_out, summary.errors
        );
    }
    println!(
        "  Total    {:>3}/{:<3} passed",
        report.totals.passed, report.totals.total
    );
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
    println!(
        "Usage: crabnes-test-rom [--max-cycles N] [--max-resets N] [--json report.json] <test.nes|directory>..."
    );
    println!("Runs ROMs that implement the standard $6000 blargg test protocol.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_common_accuracy_test_names() {
        assert_eq!(classify_test(Path::new("tests/cpu/instr_test.nes")), "CPU");
        assert_eq!(classify_test(Path::new("tests/ppu/sprite_hit.nes")), "PPU");
        assert_eq!(classify_test(Path::new("tests/apu/dmc_dma.nes")), "APU");
        assert_eq!(classify_test(Path::new("tests/mmc3/irq.nes")), "Mapper");
    }

    #[test]
    fn report_summarizes_each_category() {
        let make = |category: &str, status| AccuracyRecord {
            path: format!("{category}.nes"),
            category: category.into(),
            status,
            failure_code: None,
            message: String::new(),
            cpu_cycles: 0,
            instructions: 0,
            resets: 0,
        };
        let report = build_accuracy_report(vec![
            make("CPU", RecordStatus::Passed),
            make("CPU", RecordStatus::Failed),
            make("PPU", RecordStatus::Passed),
        ]);
        assert!(!report.all_passed);
        assert_eq!(report.totals.total, 3);
        assert_eq!(report.totals.passed, 2);
        assert_eq!(report.categories["CPU"].failed, 1);
    }
}
