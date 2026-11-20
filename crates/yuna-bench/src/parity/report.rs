//! The parity run's report, in Markdown: the verdict first, then what is worse or void, then
//! every figure.

use std::fmt::Write as _;

use super::record::{FigureRecord, RunFile};

/// The report of `file`.
pub fn markdown(file: &RunFile) -> String {
    let mut text = String::new();
    let verdict = match file.exit {
        Some(0) => "**Parity holds**: no figure is worse, none void.",
        Some(1) => "**Parity fails**: a figure is worse.",
        Some(2) => "**Parity unknown**: a figure is void, or the run failed.",
        _ => "**Running**: not every run has ended.",
    };
    let _ = writeln!(text, "# Parity, tier {}\n\n{verdict}\n", file.tier);
    for side in &file.sides {
        let commit = side.commit.as_deref().map_or_else(
            || "no commit".to_owned(),
            |c| c[..c.len().min(12)].to_owned(),
        );
        let dirty = if side.dirty == Some(true) {
            ", with changes"
        } else {
            ""
        };
        let canary = side
            .canary
            .as_deref()
            .map(|c| format!(", canary `{c}`"))
            .unwrap_or_default();
        let _ = writeln!(
            text,
            "- **{}**: `{}` ({commit}{dirty}{canary}); {}; Mesa {}{}",
            side.side,
            side.asked,
            side.rustc.as_deref().unwrap_or("toolchain unknown"),
            side.mesa
                .as_deref()
                .and_then(|m| crate::procfs::store_version(m))
                .unwrap_or("unknown"),
            side.error
                .as_deref()
                .map(|e| format!("; **its build failed**: {e}"))
                .unwrap_or_default(),
        );
    }
    let _ = writeln!(
        text,
        "- {} runs, seed {}, valgrind {}; {}\n",
        file.runs.len(),
        file.seed,
        file.valgrind.as_deref().unwrap_or("unknown"),
        file.machine
            .cpu_model
            .as_deref()
            .unwrap_or("processor unknown"),
    );
    for note in &file.notes {
        let _ = writeln!(text, "- {note}");
    }
    if !file.notes.is_empty() {
        text.push('\n');
    }
    let failed: Vec<_> = file.runs.iter().filter(|run| run.error.is_some()).collect();
    if !failed.is_empty() {
        let _ = writeln!(text, "## Runs that failed\n");
        for run in failed {
            let _ = writeln!(
                text,
                "- `{}`: {}",
                run.label,
                run.error.as_deref().unwrap_or_default()
            );
        }
        text.push('\n');
    }
    let flagged: Vec<&FigureRecord> = file
        .figures
        .iter()
        .filter(|f| matches!(f.verdict.as_str(), "worse" | "void"))
        .collect();
    if !flagged.is_empty() {
        let _ = writeln!(text, "## Worse or void\n");
        table(&mut text, &flagged);
    }
    if let Some(check) = &file.move_check {
        let _ = writeln!(
            text,
            "## The baseline's move, from `{}`\n",
            &check.old[..check.old.len().min(12)]
        );
        for figure in &check.worse {
            let _ = writeln!(
                text,
                "- {figure}: worse than the old baseline, named by the move"
            );
        }
        for figure in &check.unnamed {
            let _ = writeln!(
                text,
                "- **{figure}**: worse than the old baseline, or not measured \
                                    against it, and named by no move"
            );
        }
        if check.worse.is_empty() && check.unnamed.is_empty() {
            let _ = writeln!(text, "- no figure worse than the old baseline");
        }
        text.push('\n');
    }
    if !file.pushes.is_empty() {
        let _ = writeln!(text, "## Which push moved them\n");
        for push in &file.pushes {
            let _ = writeln!(text, "- {}: {}", push.figure, push.said);
        }
        text.push('\n');
    }
    let _ = writeln!(text, "## Every figure\n");
    table(&mut text, &file.figures.iter().collect::<Vec<_>>());
    text
}

fn table(text: &mut String, figures: &[&FigureRecord]) {
    let _ = writeln!(
        text,
        "| figure | class | base | head | change | allowed | spread | verdict |\n\
         |---|---|---:|---:|---:|---:|---:|---|"
    );
    for figure in figures {
        let change = match (figure.base, figure.head) {
            (Some(base), Some(head)) if base != 0.0 => {
                format!("{:+.2} %", (head - base) / base.abs() * 100.0)
            }
            (Some(base), Some(head)) => format!("{:+}", number(head - base)),
            _ => "—".to_owned(),
        };
        let allowed = match figure.class.as_str() {
            "parity" => {
                let relative = figure.threshold_relative * 100.0;
                format!("{relative:.2} %, ≥ {}", number(figure.threshold_absolute))
            }
            "budget" => figure
                .budget
                .map(|b| format!("≤ {}", number(b)))
                .unwrap_or_default(),
            _ => "—".to_owned(),
        };
        let spread = figure
            .spread
            .map_or_else(|| "—".to_owned(), |s| format!("{:.3} %", s * 100.0));
        let verdict = match &figure.reason {
            Some(reason) => format!("{}: {}", figure.verdict, reason.replace('|', "\\|")),
            None => figure.verdict.clone(),
        };
        let value = |v: Option<f64>| v.map_or_else(|| "—".to_owned(), number);
        let _ = writeln!(
            text,
            "| {} | {} | {} | {} | {change} | {allowed} | {spread} | {verdict} |",
            figure.name,
            figure.class,
            value(figure.base),
            value(figure.head),
        );
    }
    text.push('\n');
}

/// A value as a reader takes it in: whole above 100, else two decimals.
fn number(value: f64) -> String {
    if value.abs() >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_puts_what_is_worse_or_void_first() {
        let file = RunFile {
            tier: "ci".to_owned(),
            exit: Some(1),
            figures: vec![
                FigureRecord {
                    name: "partial: instructions a frame, yuna".to_owned(),
                    class: "parity".to_owned(),
                    threshold_relative: 0.005,
                    threshold_absolute: 1000.0,
                    base: Some(180_000.0),
                    head: Some(190_000.0),
                    verdict: "worse".to_owned(),
                    ..FigureRecord::default()
                },
                FigureRecord {
                    name: "partial: Pss_File".to_owned(),
                    class: "recorded".to_owned(),
                    base: Some(9000.0),
                    head: Some(9100.0),
                    verdict: "recorded".to_owned(),
                    ..FigureRecord::default()
                },
                FigureRecord {
                    name: "operations: *".to_owned(),
                    class: "parity".to_owned(),
                    verdict: "void".to_owned(),
                    reason: Some("base: its tree has no examples/operations.rs".to_owned()),
                    ..FigureRecord::default()
                },
            ],
            ..RunFile::default()
        };
        let text = markdown(&file);
        assert!(text.contains("**Parity fails**"), "{text}");
        let worse = text.find("## Worse or void").unwrap();
        let every = text.find("## Every figure").unwrap();
        assert!(worse < every);
        assert!(text[worse..every].contains("+5.56 %"), "{text}");
        assert!(!text[worse..every].contains("Pss_File"));
        assert!(text[worse..every].contains("its tree has no"), "{text}");
    }
}
