//! `docs/protocols.md` is true of the globals Yuna announces
//! ([ADR-0058](../../../../docs/adr/0058-what-yuna-takes-from-the-owners-other-projects.md)):
//! the rows `Offered` name every global a client of the owner's seat and one of an agent's seat
//! are shown, and no other. A row says to whom before any `;`: `the owner's seat alone`, `agents' seats alone`,
//! `Xwayland alone`, never shown to either here, or `when the renderer imports a format`, shown
//! or not as the renderer can.

use std::collections::BTreeSet;
use std::path::Path;

use yuna::OutputMode;

use crate::client::Client;
use crate::{manual_compositor, run_armed_deadlines};

/// Who a row `Offered` is shown to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShownTo {
    Everyone,
    OwnersSeat,
    AgentsSeats,
    Xwayland,
    /// Every client, when the renderer imports a dmabuf format.
    IfTheRendererImports,
}

/// Each global the rows `Offered` name, with whom it is shown to.
fn offered_globals() -> Vec<(String, ShownTo)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/protocols.md");
    let text = std::fs::read_to_string(&path).expect("docs/protocols.md");
    let mut offered = Vec::new();
    for line in text.lines() {
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        let [_, globals, yuna, ..] = cells[..] else {
            continue;
        };
        if !yuna.starts_with("Offered") {
            continue;
        }
        // To whom is said first; what follows a `;` or a link is commentary.
        let yuna = yuna.split([';', '(']).next().unwrap_or(yuna);
        let shown_to = if yuna.contains("the owner's seat alone") {
            ShownTo::OwnersSeat
        } else if yuna.contains("agents' seats alone") {
            ShownTo::AgentsSeats
        } else if yuna.contains("Xwayland alone") {
            ShownTo::Xwayland
        } else if yuna.contains("when the renderer imports") {
            ShownTo::IfTheRendererImports
        } else {
            ShownTo::Everyone
        };
        let names: Vec<String> = globals
            .split(',')
            .map(|name| name.trim().trim_matches('`').to_owned())
            .filter(|name| !name.is_empty())
            .collect();
        assert!(!names.is_empty(), "an offered row names no global: {line}");
        offered.extend(names.into_iter().map(|name| (name, shown_to)));
    }
    offered
}

fn interfaces(client: &Client) -> BTreeSet<String> {
    client
        .state
        .announced_globals
        .iter()
        .map(|(_, interface)| interface.clone())
        .collect()
}

#[test]
fn the_offered_rows_are_the_globals_announced() {
    let mut compositor = manual_compositor();
    let agent = compositor.add_seat().expect("a seat");
    let mode = OutputMode::new(1920, 1080, 60.0, 1.0).expect("a mode");
    compositor.add_output(agent, mode).expect("an output");
    run_armed_deadlines(&mut compositor);
    let owner = Client::connect(&mut compositor);
    let on_agent = Client::connect_to(&mut compositor, agent);
    let offered = offered_globals();
    let dmabuf_offered = compositor.dmabuf_format_count() > 0;

    for (seat, client, own) in [
        ("the owner's", &owner, ShownTo::OwnersSeat),
        ("an agent's", &on_agent, ShownTo::AgentsSeats),
    ] {
        let expected: BTreeSet<String> = offered
            .iter()
            .filter(|(_, shown_to)| match shown_to {
                ShownTo::Everyone => true,
                ShownTo::IfTheRendererImports => dmabuf_offered,
                ShownTo::Xwayland => false,
                kind => *kind == own,
            })
            .map(|(name, _)| name.clone())
            .collect();
        let announced = interfaces(client);
        assert_eq!(
            announced, expected,
            "{seat} seat: the globals announced differ from docs/protocols.md's rows Offered"
        );
    }
}
