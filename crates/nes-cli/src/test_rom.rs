use nes_core::{CPU_CLOCK_HZ, EmulationError, Nes};

const STATUS_ADDRESS: u16 = 0x6000;
const SIGNATURE_ADDRESS: u16 = 0x6001;
const MESSAGE_ADDRESS: u16 = 0x6004;
const SIGNATURE: [u8; 3] = [0xde, 0xb0, 0x61];
const STATUS_RUNNING: u8 = 0x80;
const STATUS_NEEDS_RESET: u8 = 0x81;
const MAX_MESSAGE_BYTES: usize = 4096;

pub const DEFAULT_MAX_CYCLES: u64 = CPU_CLOCK_HZ as u64 * 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TestOptions {
    pub max_cycles: u64,
    pub max_resets: u32,
}

impl Default for TestOptions {
    fn default() -> Self {
        Self {
            max_cycles: DEFAULT_MAX_CYCLES,
            max_resets: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestOutcome {
    Passed,
    Failed(u8),
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestReport {
    pub outcome: TestOutcome,
    pub message: String,
    pub cpu_cycles: u64,
    pub instructions: u64,
    pub resets: u32,
}

/// Run a ROM that implements the common blargg `$6000` test protocol.
///
/// The runner waits for the `DE B0 61` signature at `$6001`, observes `$80` at
/// `$6000` to arm the protocol, optionally services `$81` reset requests, and
/// treats `$00` as success or any other terminal byte as a failure code.
pub fn run_test_rom(rom: &[u8], options: TestOptions) -> Result<TestReport, EmulationError> {
    let mut nes = Nes::from_ines(rom)?;
    let mut cpu_cycles = 0_u64;
    let mut instructions = 0_u64;
    let mut resets = 0_u32;
    let mut protocol_armed = false;
    let mut waiting_for_reset_ack = false;

    while cpu_cycles < options.max_cycles {
        let cycles = nes.step_instruction()?;
        if cycles == 0 {
            break;
        }
        cpu_cycles = cpu_cycles.saturating_add(u64::from(cycles));
        instructions = instructions.saturating_add(1);

        if !has_signature(&nes) {
            continue;
        }

        let status = nes.peek_cpu(STATUS_ADDRESS);
        if status == STATUS_RUNNING {
            protocol_armed = true;
            waiting_for_reset_ack = false;
            continue;
        }

        if status == STATUS_NEEDS_RESET {
            protocol_armed = true;
            if !waiting_for_reset_ack && resets < options.max_resets {
                nes.reset();
                resets += 1;
                waiting_for_reset_ack = true;
            }
            continue;
        }

        waiting_for_reset_ack = false;
        if protocol_armed {
            return Ok(TestReport {
                outcome: if status == 0 {
                    TestOutcome::Passed
                } else {
                    TestOutcome::Failed(status)
                },
                message: read_message(&nes),
                cpu_cycles,
                instructions,
                resets,
            });
        }
    }

    Ok(TestReport {
        outcome: TestOutcome::TimedOut,
        message: if protocol_armed {
            read_message(&nes)
        } else {
            "test protocol signature was not observed".into()
        },
        cpu_cycles,
        instructions,
        resets,
    })
}

fn has_signature(nes: &Nes) -> bool {
    SIGNATURE
        .iter()
        .enumerate()
        .all(|(offset, expected)| nes.peek_cpu(SIGNATURE_ADDRESS + offset as u16) == *expected)
}

fn read_message(nes: &Nes) -> String {
    let bytes: Vec<_> = (0..MAX_MESSAGE_BYTES)
        .map(|offset| nes.peek_cpu(MESSAGE_ADDRESS.wrapping_add(offset as u16)))
        .take_while(|byte| *byte != 0)
        .collect();
    String::from_utf8_lossy(&bytes)
        .trim_matches(|character: char| character.is_ascii_whitespace())
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nrom_test_rom(program: &[u8]) -> Vec<u8> {
        let mut rom = vec![0; 16 + 0x4000];
        rom[0..4].copy_from_slice(b"NES\x1a");
        rom[4] = 1;
        rom[16..16 + program.len()].copy_from_slice(program);
        rom[16 + 0x3ffa..16 + 0x4000].copy_from_slice(&[0x00, 0x80, 0x00, 0x80, 0x00, 0x80]);
        rom
    }

    fn protocol_program(terminal_status: u8) -> Vec<u8> {
        let mut program = vec![
            0xa9,
            0x80,
            0x8d,
            0x00,
            0x60, // LDA #$80; STA $6000
            0xa9,
            0xde,
            0x8d,
            0x01,
            0x60, // signature
            0xa9,
            0xb0,
            0x8d,
            0x02,
            0x60,
            0xa9,
            0x61,
            0x8d,
            0x03,
            0x60,
            0xa9,
            b'O',
            0x8d,
            0x04,
            0x60, // message "OK"
            0xa9,
            b'K',
            0x8d,
            0x05,
            0x60,
            0xa9,
            0x00,
            0x8d,
            0x06,
            0x60,
            0xa9,
            terminal_status,
            0x8d,
            0x00,
            0x60,
        ];
        let loop_address = 0x8000_u16 + program.len() as u16;
        program.extend_from_slice(&[0x4c, loop_address as u8, (loop_address >> 8) as u8]);
        program
    }

    #[test]
    fn reports_a_passing_protocol_rom() {
        let rom = nrom_test_rom(&protocol_program(0));
        let report = run_test_rom(
            &rom,
            TestOptions {
                max_cycles: 1_000,
                ..TestOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report.outcome, TestOutcome::Passed);
        assert_eq!(report.message, "OK");
    }

    #[test]
    fn preserves_the_failure_code_and_message() {
        let rom = nrom_test_rom(&protocol_program(3));
        let report = run_test_rom(
            &rom,
            TestOptions {
                max_cycles: 1_000,
                ..TestOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report.outcome, TestOutcome::Failed(3));
        assert_eq!(report.message, "OK");
    }

    #[test]
    fn times_out_when_a_rom_does_not_publish_the_protocol() {
        let rom = nrom_test_rom(&[0x4c, 0x00, 0x80]);
        let report = run_test_rom(
            &rom,
            TestOptions {
                max_cycles: 30,
                ..TestOptions::default()
            },
        )
        .unwrap();
        assert_eq!(report.outcome, TestOutcome::TimedOut);
        assert!(report.message.contains("signature"));
    }
}
