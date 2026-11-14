//! The special workspace's fade, in when it is shown and out when it is hidden
//! ([ADR-0055](../../../../docs/adr/0055-what-moves-first-and-animations-off-headless.md)).

use std::time::Duration;

use yuna_layout::{Placed, WorkspaceId};

use crate::{Curve, CurveParams};

/// A special workspace's fade, while it runs.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SpecialFade {
    /// The special workspace.
    pub workspace: WorkspaceId,
    /// Its opacity: toward 1 while it is shown, toward 0 once hidden.
    alpha: Curve,
    /// While it fades out, its windows where they were when it was hidden, bottom to top: the
    /// layout no longer places them. Empty while it fades in.
    pub hidden: Vec<Placed>,
}

impl SpecialFade {
    /// The fade of `workspace` coming in at `now`: from 0, or on from `before`, its fade until
    /// now if the output drew it already, which a toggle back mid-fade reverses.
    pub fn showing(
        workspace: WorkspaceId,
        before: Option<Curve>,
        now: Duration,
        params: CurveParams,
    ) -> SpecialFade {
        let alpha = match before {
            Some(alpha) if alpha.target() == 1.0 => alpha,
            Some(alpha) => alpha.retarget(now, 1.0),
            None => Curve::new(params, 0.0, 1.0, now),
        };
        SpecialFade {
            workspace,
            alpha,
            hidden: Vec::new(),
        }
    }

    /// The fade of `workspace` going out at `now`, on from `before`, its fade until now, its
    /// windows drawn at `hidden`.
    pub fn hiding(
        workspace: WorkspaceId,
        before: Curve,
        hidden: Vec<Placed>,
        now: Duration,
    ) -> SpecialFade {
        let alpha = if before.target() == 0.0 {
            before
        } else {
            before.retarget(now, 0.0)
        };
        SpecialFade {
            workspace,
            alpha,
            hidden,
        }
    }

    /// The opacity at `at`, within 0 and 1: a Bézier easing may take the curve beyond.
    pub fn alpha(&self, at: Duration) -> f64 {
        self.alpha.value(at).clamp(0.0, 1.0)
    }

    /// The opacity's curve.
    pub fn curve(&self) -> Curve {
        self.alpha
    }

    /// Whether it has ended at `at`.
    pub fn is_done(&self, at: Duration) -> bool {
        self.alpha.is_done(at)
    }

    /// When it ends.
    pub fn end(&self) -> Duration {
        self.alpha.end()
    }
}
