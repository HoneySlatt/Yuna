//! The `outputs` and `workspaces` tools: a seat's virtual outputs listed, created, configured and
//! removed ([ADR-0049](../../../../docs/adr/0049-vrr-and-hdr-outputs-through-mcp.md),
//! [ADR-0090](../../../../docs/adr/0090-an-outputs-mode-and-scale-changed-whole-through-mcp.md)),
//! and its workspaces listed, created, removed, shown, and given a window.
//!
//! An output's mode is read here from a call's values, `seat create`'s too (`seats.rs`). Its
//! neighbours: `mod.rs` holds the server; `tools/` the tools' schemas, arguments and results.

use serde_json::{Map, Value, json};
use yuna_layout::{OutputId, SeatId, WindowId};

use super::Server;
use super::tools::{self, OutputsCall, ToolResult, WorkspacesCall};
use crate::colour::description::PG32UCDM;
use crate::output::{InvalidOutput, OutputMode};

impl Server {
    /// An output mode from a call's values, each defaulting to the seats' default output's, its
    /// HDR monitor included (ADR-0049). Its VRR range's least rate is the default only when the
    /// call gives no rate either: a range belongs with its rate, and a call that names its own
    /// rate asked for no VRR. `hdr: true` announces the PG32UCDM's colours, the one monitor Yuna
    /// knows; `false` none.
    pub(super) fn mode_of(
        &self,
        width: Option<u32>,
        height: Option<u32>,
        rate: Option<f64>,
        scale: Option<f64>,
        vrr_min: Option<f64>,
        hdr: Option<bool>,
    ) -> Result<OutputMode, ToolResult> {
        let default = self.seat_output;
        let default_vrr = rate.is_none().then(|| default.vrr_min()).flatten();
        checked_mode(
            OutputMode::new(
                width.unwrap_or(default.width()),
                height.unwrap_or(default.height()),
                rate.unwrap_or(default.rate()),
                scale.unwrap_or(default.scale()),
            ),
            vrr_min.or(default_vrr),
            hdr,
            default,
        )
    }

    /// The mode `outputs configure` gives an output whose mode is `own`: each value the call
    /// leaves out is the output's own, its VRR range included when the call gives another rate,
    /// so that a change of rate alone keeps VRR on; `vrr_min: 0` runs it at a fixed rate; and
    /// `hdr` as [`Server::mode_of`] reads it.
    fn configured_mode(
        own: OutputMode,
        width: Option<u32>,
        height: Option<u32>,
        rate: Option<f64>,
        scale: Option<f64>,
        vrr_min: Option<f64>,
        hdr: Option<bool>,
    ) -> Result<OutputMode, ToolResult> {
        checked_mode(
            OutputMode::new(
                width.unwrap_or(own.width()),
                height.unwrap_or(own.height()),
                rate.unwrap_or(own.rate()),
                scale.unwrap_or(own.scale()),
            ),
            match vrr_min {
                Some(0.0) => None,
                Some(min) => Some(min),
                None => own.vrr_min(),
            },
            hdr,
            own,
        )
    }

    /// The mode of output `output` of `seat`; none for an unknown one or another seat's.
    fn output_mode(&self, seat: SeatId, output: OutputId) -> Option<OutputMode> {
        self.compositor
            .output_descriptions()
            .iter()
            .find(|description| description.id == output && description.seat == seat)
            .map(|description| description.mode)
    }

    /// The output `output` of `seat`, described; none for an unknown one or another seat's.
    fn output_of(&self, seat: SeatId, output: OutputId) -> Option<Value> {
        self.compositor
            .output_descriptions()
            .iter()
            .find(|description| description.id == output && description.seat == seat)
            .map(tools::output_json)
    }

    pub(super) fn outputs(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        match call {
            OutputsCall::List { seat } => {
                let seat = match self.agent_seat(seat) {
                    Ok(seat) => seat,
                    Err(failed) => return failed,
                };
                let outputs: Vec<Value> = self
                    .compositor
                    .output_descriptions()
                    .iter()
                    .filter(|output| output.seat == seat)
                    .map(tools::output_json)
                    .collect();
                ToolResult::done(json!({ "outputs": outputs }))
            }
            OutputsCall::Create {
                seat,
                width,
                height,
                rate,
                scale,
                vrr_min,
                hdr,
            } => {
                let seat = match self.agent_seat(seat) {
                    Ok(seat) => seat,
                    Err(failed) => return failed,
                };
                let mode = match self.mode_of(width, height, rate, scale, vrr_min, hdr) {
                    Ok(mode) => mode,
                    Err(failed) => return failed,
                };
                match self.compositor.add_output(seat, mode) {
                    Ok(id) => ToolResult::done(json!({ "output": self.output_of(seat, id) })),
                    Err(error) => ToolResult::failed(error.to_string()),
                }
            }
            OutputsCall::Configure {
                seat,
                output,
                width,
                height,
                rate,
                scale,
                vrr_min,
                hdr,
            } => {
                let seat = match self.agent_seat(seat) {
                    Ok(seat) => seat,
                    Err(failed) => return failed,
                };
                let id = OutputId(output);
                let Some(own) = self.output_mode(seat, id) else {
                    return ToolResult::failed(format!("no output {output} on seat {}", seat.0));
                };
                let mode =
                    match Server::configured_mode(own, width, height, rate, scale, vrr_min, hdr) {
                        Ok(mode) => mode,
                        Err(failed) => return failed,
                    };
                match self.compositor.configure_output(id, mode) {
                    Ok(()) => ToolResult::done(json!({ "output": self.output_of(seat, id) })),
                    Err(error) => ToolResult::failed(error.to_string()),
                }
            }
            OutputsCall::Remove { seat, output } => {
                let seat = match self.agent_seat(seat) {
                    Ok(seat) => seat,
                    Err(failed) => return failed,
                };
                if self.output_of(seat, OutputId(output)).is_none() {
                    return ToolResult::failed(format!("no output {output} on seat {}", seat.0));
                }
                match self.compositor.remove_output(OutputId(output)) {
                    Ok(()) => ToolResult::done(json!({ "removed": output })),
                    Err(error) => ToolResult::failed(error.to_string()),
                }
            }
        }
    }

    pub(super) fn workspaces(&mut self, arguments: Map<String, Value>) -> ToolResult {
        let call: WorkspacesCall = match tools::arguments(arguments) {
            Ok(call) => call,
            Err(reason) => return ToolResult::failed(reason),
        };
        let seat = match &call {
            WorkspacesCall::List { seat }
            | WorkspacesCall::Create { seat, .. }
            | WorkspacesCall::Remove { seat, .. }
            | WorkspacesCall::Focus { seat, .. }
            | WorkspacesCall::MoveWindow { seat, .. } => *seat,
        };
        let seat = match self.agent_seat(seat) {
            Ok(seat) => seat,
            Err(failed) => return failed,
        };
        let changed = match call {
            WorkspacesCall::List { .. } => Ok(()),
            WorkspacesCall::Create { name, output, .. } => {
                let output = output.map(OutputId);
                if let Some(output) = output
                    && self.output_of(seat, output).is_none()
                {
                    return ToolResult::failed(format!(
                        "no output {} on seat {}",
                        output.0, seat.0
                    ));
                }
                self.compositor
                    .change_layout(|layout| layout.add_workspace(seat, &name, output))
            }
            WorkspacesCall::Remove { name, .. } => self
                .compositor
                .change_layout(|layout| layout.remove_workspace(seat, &name)),
            WorkspacesCall::Focus { name, .. } => self
                .compositor
                .change_layout(|layout| layout.focus_workspace(seat, &name)),
            WorkspacesCall::MoveWindow {
                window, workspace, ..
            } => {
                let window = WindowId(window);
                let ours = self
                    .compositor
                    .windows()
                    .iter()
                    .any(|description| description.id == window && description.seat == seat);
                if !ours {
                    return ToolResult::failed(format!(
                        "no window {} on seat {}",
                        window.0, seat.0
                    ));
                }
                self.compositor.change_layout(|layout| {
                    layout.focus_window(window)?;
                    layout.move_to_workspace(seat, &workspace)
                })
            }
        };
        if let Err(error) = changed {
            return ToolResult::failed(error.to_string());
        }
        let outputs: Vec<Value> = self
            .compositor
            .output_descriptions()
            .iter()
            .filter(|output| output.seat == seat)
            .map(|output| {
                let workspaces: Vec<Value> = self
                    .compositor
                    .layout()
                    .workspaces(output.id)
                    .unwrap_or_default()
                    .iter()
                    .map(tools::workspace_json)
                    .collect();
                json!({ "output": output.id.0, "workspaces": workspaces })
            })
            .collect();
        ToolResult::done(json!({ "outputs": outputs }))
    }
}

/// `mode`, a mode just made, under VRR from `vrr_min` and announcing what `hdr` says: `true`
/// the PG32UCDM's colours, the one monitor Yuna knows, `false` none, and left out
/// `default`'s. A mode refused on the way is the call's failure, with its reason.
fn checked_mode(
    mode: Result<OutputMode, InvalidOutput>,
    vrr_min: Option<f64>,
    hdr: Option<bool>,
    default: OutputMode,
) -> Result<OutputMode, ToolResult> {
    let refused = |reason: InvalidOutput| ToolResult::failed(reason.to_string());
    let mut mode = mode.map_err(refused)?;
    if let Some(min) = vrr_min {
        mode = mode.with_vrr(min).map_err(refused)?;
    }
    let monitor = match hdr {
        Some(true) => Some(PG32UCDM),
        Some(false) => None,
        None => default.hdr(),
    };
    if let Some(monitor) = monitor {
        mode = mode.with_hdr(monitor);
    }
    Ok(mode)
}
