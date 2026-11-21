//! The parity run's report, in Markdown: the verdict first, then what is worse or void, then
//! every figure.

use std::fmt::Write as _;

use super::record::{FigureRecord, RunFile};

/// The report of `file`.
pub fn markdown(file: &RunFile) -> String {
    let mut text = String::new();
    let verdict = match file.exit {
        Some(0) => "**Parity holds**: no figure is worse, none void or inconclusive.",
        Some(1) => "**Parity fails**: a figure is worse.",
        Some(2) => "**Parity unknown**: a figure is void or inconclusive, or the run failed.",
        _ => "**Running**: not every run has ended.",
    };
    let _ = writeln!(text, "# Parity, tier {}\n\n{verdict}\n", file.tier);
    if !file.gpu_load_unread.is_empty() {
        let _ = writeln!(
            text,
            "**Caveat**: the other programs' GPU load could not be read in some timed runs ({}): \
             a game on the GPU would not have marked them disturbed. On the GPU, such a case is \
             void unless `--gpu-load-unknown-ok` was given.\n",
            file.gpu_load_unread.join("; ")
        );
    }
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
    let disturbed = file
        .runs
        .iter()
        .filter(|run| run.disturbed.is_some())
        .count();
    if disturbed > 0 {
        let _ = writeln!(
            text,
            "{disturbed} timed runs were disturbed, their pairs dropped and replaced: the run \
             file says why for each.\n"
        );
    }
    let flagged: Vec<&FigureRecord> = file
        .figures
        .iter()
        .filter(|f| matches!(f.verdict.as_str(), "worse" | "void" | "inconclusive"))
        .collect();
    if !flagged.is_empty() {
        let _ = writeln!(text, "## Worse, void or inconclusive\n");
        tables(&mut text, &flagged);
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
    tables(&mut text, &file.figures.iter().collect::<Vec<_>>());
    if let Some(soak) = &file.soak {
        let last = |readings: &[(f64, f64)]| {
            readings.last().map_or_else(
                || "none".to_owned(),
                |(at, kib)| format!("{kib:.0} KiB at {at:.1} min"),
            )
        };
        let _ = writeln!(
            text,
            "## The soak\n\n{} instances a side, {} s: the base from {} to {}, the head from {} \
             to {}.\n",
            soak.instances,
            soak.seconds,
            soak.base.first().map_or(0.0, |(_, kib)| *kib),
            last(&soak.base),
            soak.head.first().map_or(0.0, |(_, kib)| *kib),
            last(&soak.head),
        );
        for error in &soak.errors {
            let _ = writeln!(text, "- {error}");
        }
    }
    text
}

/// The tables of `figures`: the timed tier's apart, judged over pairs.
fn tables(text: &mut String, figures: &[&FigureRecord]) {
    let (timed, others): (Vec<&FigureRecord>, Vec<&FigureRecord>) =
        figures.iter().partition(|figure| figure.timed);
    if !others.is_empty() {
        table(text, &others);
    }
    if !timed.is_empty() {
        timed_table(text, &timed);
    }
}

/// The timed tier's figures: for a figure judged over pairs, the ratio of head to base, its
/// one-sided 95 % bounds and the deviation of its logs, the spread its threshold comes from.
fn timed_table(text: &mut String, figures: &[&FigureRecord]) {
    let _ = writeln!(
        text,
        "| figure | class | base | head | head / base, 95 % bounds | allowed | pairs (dropped) | \
         σ of ln | verdict |\n|---|---|---:|---:|---|---:|---:|---:|---|"
    );
    let percent = |log: f64| format!("{:+.2} %", (log.exp() - 1.0) * 100.0);
    for figure in figures {
        let value = |v: Option<f64>| v.map_or_else(|| "—".to_owned(), number);
        let (bounds, pairs, deviation) = match &figure.judgement {
            // A budget judges the head's longest value, not the ratio: its pairs alone.
            Some(j) if figure.class == "budget" => (
                "—".to_owned(),
                format!("{} ({})", j.count, j.dropped),
                "—".to_owned(),
            ),
            Some(j) if j.on.starts_with("ln") => (
                format!(
                    "{} ({} to {})",
                    percent(j.mean),
                    percent(j.lower),
                    percent(j.upper)
                ),
                format!("{} ({})", j.count, j.dropped),
                format!("{:.2} %", j.deviation * 100.0),
            ),
            Some(j) => (
                format!(
                    "{}: {} ({} to {}) {}",
                    j.on,
                    number(j.mean),
                    number(j.lower),
                    number(j.upper),
                    figure.unit
                ),
                format!("{} readings", j.count),
                "—".to_owned(),
            ),
            None => ("—".to_owned(), "—".to_owned(), "—".to_owned()),
        };
        let allowed = match figure.class.as_str() {
            "parity" if figure.unit == "KiB a minute" => {
                format!("≤ {}", number(figure.threshold_absolute))
            }
            "parity" => format!("+{:.2} %", figure.threshold_relative * 100.0),
            "budget" => figure
                .budget
                .map(|b| format!("≤ {}", number(b)))
                .unwrap_or_default(),
            _ => "—".to_owned(),
        };
        let verdict = match &figure.reason {
            Some(reason) => format!("{}: {}", figure.verdict, reason.replace('|', "\\|")),
            None => figure.verdict.clone(),
        };
        let _ = writeln!(
            text,
            "| {} | {} | {} | {} | {bounds} | {allowed} | {pairs} | {deviation} | {verdict} |",
            figure.name,
            figure.class,
            value(figure.base),
            value(figure.head),
        );
    }
    text.push('\n');
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
        let worse = text.find("## Worse, void or inconclusive").unwrap();
        let every = text.find("## Every figure").unwrap();
        assert!(worse < every);
        assert!(text[worse..every].contains("+5.56 %"), "{text}");
        assert!(!text[worse..every].contains("Pss_File"));
        assert!(text[worse..every].contains("its tree has no"), "{text}");
    }

    #[test]
    fn a_timed_figure_shows_its_ratio_its_bounds_and_its_pairs() {
        let file = RunFile {
            tier: "timed".to_owned(),
            exit: Some(2),
            figures: vec![FigureRecord {
                name: "partial: a frame's drawing time".to_owned(),
                unit: "µs".to_owned(),
                class: "parity".to_owned(),
                threshold_relative: 0.05,
                base: Some(180.0),
                head: Some(181.0),
                verdict: "inconclusive".to_owned(),
                timed: true,
                judgement: Some(super::super::record::JudgementRecord {
                    on: "ln(head / base)".to_owned(),
                    count: 30,
                    dropped: 2,
                    mean: 0.01_f64.ln_1p(),
                    deviation: 0.04,
                    lower: -0.01,
                    upper: 0.06,
                }),
                ..FigureRecord::default()
            }],
            ..RunFile::default()
        };
        let text = markdown(&file);
        assert!(text.contains("**Parity unknown**"), "{text}");
        assert!(!text.contains("**Caveat**"), "{text}");
        let unread = RunFile {
            gpu_load_unread: vec!["partial: 22 of 22 runs".to_owned()],
            ..file.clone()
        };
        let caveat = markdown(&unread);
        let headline = caveat.find("**Caveat**").expect("the caveat");
        assert!(
            headline < caveat.find("## ").unwrap(),
            "in the headline: {caveat}"
        );
        assert!(caveat.contains("(partial: 22 of 22 runs)"), "{caveat}");
        let flagged = text.find("## Worse, void or inconclusive").unwrap();
        let row = &text[flagged..];
        assert!(row.contains("+1.00 % (-1.00 % to +6.18 %)"), "{text}");
        assert!(
            row.contains("| +5.00 % | 30 (2) | 4.00 % | inconclusive |"),
            "{text}"
        );
    }
}
