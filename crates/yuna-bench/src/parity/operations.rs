//! The layout's and motion's operations, counted by callgrind: each case of a build's
//! `operations` and `motion_operations` programs (`crates/yuna-layout/examples/operations.rs`,
//! `crates/yuna-motion/examples/motion_operations.rs`), run twice, `n` and `2n` repeats, so that
//! the difference over `n` is one operation's instructions, the setup left out.
//!
//! Each run's digest says what the operations did; two builds whose digests differ did not do
//! the same work, and the figure is void.

use std::fs::{self, File};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use super::scenario::Outcome;
use super::valgrind::{self, Tool};

/// The repeats of the shorter run of each program's cases; the longer makes twice as many.
pub fn repeats(program: &str) -> u64 {
    match program {
        // An update at 10,000 windows costs what 10 of the layout's operations at 1,000 do.
        "motion_operations" => 10,
        // A case's setup counts a few hundred instructions more or less from run to run, its hash
        // maps keyed at random: 50 repeats make that a few an operation.
        _ => 50,
    }
}

/// The cases `program` lists.
pub fn cases(program: &Path) -> Result<Vec<String>, String> {
    let output = Command::new(program)
        .arg("--list")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{}: {e}", program.display()))?;
    if !output.status.success() {
        return Err(format!("{} --list failed", program.display()));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Runs `case` of `program` `repeats` times under callgrind, its files in `folder`: its
/// instructions (`instructions`) and its digest.
pub fn run(program: &Path, valgrind: &Path, case: &str, repeats: u64, folder: &Path) -> Outcome {
    let started = Instant::now();
    let mut outcome = Outcome::default();
    let result = (|| -> Result<(), String> {
        fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
        let command = valgrind::command(valgrind, Tool::Callgrind, folder);
        let errors =
            File::create(folder.join("stderr.log")).map_err(|e| format!("stderr.log: {e}"))?;
        let output = Command::new(&command[0])
            .args(&command[1..])
            .arg(program)
            .arg(case)
            .arg(repeats.to_string())
            .env_clear()
            .stdin(Stdio::null())
            .stderr(errors)
            .output()
            .map_err(|e| format!("valgrind: {e}"))?;
        if !output.status.success() {
            return Err(format!("{case} ended with {}", output.status));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        outcome.digest = Some(
            stdout
                .lines()
                .find_map(|line| line.strip_prefix("digest "))
                .ok_or_else(|| format!("{case} printed no digest"))?
                .trim()
                .to_owned(),
        );
        let mut total = 0;
        let mut processes = 0;
        for file in valgrind::files(folder, Tool::Callgrind) {
            let text = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            let read =
                valgrind::read_callgrind(&text).map_err(|e| format!("{}: {e}", file.display()))?;
            if valgrind::runs(&read.command, program) {
                total += read.total;
                processes += 1;
            }
        }
        if processes != 1 {
            return Err(format!("{processes} callgrind files of {case}, not 1"));
        }
        outcome
            .readings
            .insert("instructions".to_owned(), total as f64);
        Ok(())
    })();
    if let Err(error) = result {
        outcome.error = Some(error);
    }
    outcome.seconds = started.elapsed().as_secs_f64();
    outcome
}
