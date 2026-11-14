//! Image descriptions: what a surface's or an output's colours mean, as `color-management-v1`
//! carries them, with no Wayland object ([ADR-0047](../../../../docs/adr/0047-color-management-v1-kwins-list.md)).
//!
//! Values keep the protocol's units, so that what a client sent is what tests read back:
//! chromaticities in millionths, minimum luminances in ten-thousandths of a cd/m², other
//! luminances in whole cd/m². Sums and comparisons are made in 64 bits, products of
//! coordinates in 128, so that no client's value overflows.

use std::fmt;

/// The transfer functions Yuna lists: KWin's, without `srgb`, deprecated from version 2 and a
/// cause of `surface_exists` errors for version-1 clients (ADR-0047).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferFunction {
    /// ITU-R BT.1886, a CRT's display response.
    Bt1886,
    /// A pure power of 2.2.
    Gamma22,
    /// SMPTE ST 2084, the perceptual quantizer of HDR10.
    St2084Pq,
    /// Hybrid log-gamma, ITU-R BT.2100's other HDR curve.
    Hlg,
    /// Linear over every real number.
    ExtLinear,
    /// `compound_power_2_4`, from version 2.
    CompoundPower24,
}

impl TransferFunction {
    /// Every one, in the order Yuna lists them.
    pub const ALL: [TransferFunction; 6] = [
        TransferFunction::Bt1886,
        TransferFunction::Gamma22,
        TransferFunction::St2084Pq,
        TransferFunction::Hlg,
        TransferFunction::ExtLinear,
        TransferFunction::CompoundPower24,
    ];

    /// Its value in the protocol's `transfer_function` enum.
    pub fn wire(self) -> u32 {
        match self {
            TransferFunction::Bt1886 => 1,
            TransferFunction::Gamma22 => 2,
            TransferFunction::ExtLinear => 5,
            TransferFunction::St2084Pq => 11,
            TransferFunction::Hlg => 13,
            TransferFunction::CompoundPower24 => 14,
        }
    }

    /// The one a client named, if Yuna lists it at the client's `version`.
    pub fn from_wire(value: u32, version: u32) -> Option<TransferFunction> {
        TransferFunction::ALL
            .into_iter()
            .find(|tf| tf.wire() == value && version >= tf.since())
    }

    /// The protocol's version that lists it first.
    pub fn since(self) -> u32 {
        match self {
            TransferFunction::CompoundPower24 => 2,
            _ => 1,
        }
    }

    /// Its name in the protocol.
    pub fn name(self) -> &'static str {
        match self {
            TransferFunction::Bt1886 => "bt1886",
            TransferFunction::Gamma22 => "gamma22",
            TransferFunction::St2084Pq => "st2084_pq",
            TransferFunction::Hlg => "hlg",
            TransferFunction::ExtLinear => "ext_linear",
            TransferFunction::CompoundPower24 => "compound_power_2_4",
        }
    }

    /// The luminances it implies when a client sets none: the protocol's, per transfer
    /// function, sRGB's otherwise.
    pub fn default_luminances(self) -> Luminances {
        match self {
            TransferFunction::St2084Pq => Luminances {
                min: 50,
                max: 10_000,
                reference: 203,
            },
            TransferFunction::Hlg => Luminances {
                min: 50,
                max: 1_000,
                reference: 203,
            },
            TransferFunction::Bt1886 => Luminances {
                min: 100,
                max: 100,
                reference: 100,
            },
            _ => Luminances::SRGB,
        }
    }
}

/// A named set of primaries and white point, all ten of the protocol's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedPrimaries {
    /// ITU-R BT.709 and sRGB.
    Srgb,
    /// NTSC of 1953 and PAL-M.
    PalM,
    /// ITU-R BT.601's 625 lines.
    Pal,
    /// ITU-R BT.601's 525 lines, SMPTE 170M.
    Ntsc,
    /// H.273's "generic film".
    GenericFilm,
    /// ITU-R BT.2020 and BT.2100.
    Bt2020,
    /// CIE 1931 XYZ itself.
    Cie1931Xyz,
    /// DCI-P3, with DCI's white.
    DciP3,
    /// Display P3, with D65's white.
    DisplayP3,
    /// Adobe RGB (1998).
    AdobeRgb,
}

impl NamedPrimaries {
    /// Every one, in the protocol's order.
    pub const ALL: [NamedPrimaries; 10] = [
        NamedPrimaries::Srgb,
        NamedPrimaries::PalM,
        NamedPrimaries::Pal,
        NamedPrimaries::Ntsc,
        NamedPrimaries::GenericFilm,
        NamedPrimaries::Bt2020,
        NamedPrimaries::Cie1931Xyz,
        NamedPrimaries::DciP3,
        NamedPrimaries::DisplayP3,
        NamedPrimaries::AdobeRgb,
    ];

    /// Its value in the protocol's `primaries` enum: 1 to 10, in order.
    pub fn wire(self) -> u32 {
        // ALL holds every variant, in the enum's order.
        NamedPrimaries::ALL
            .iter()
            .position(|p| *p == self)
            .map_or(0, |i| i as u32 + 1)
    }

    /// The set a client named, if it is one of the protocol's.
    pub fn from_wire(value: u32) -> Option<NamedPrimaries> {
        NamedPrimaries::ALL.into_iter().find(|p| p.wire() == value)
    }

    /// Its name in the protocol.
    pub fn name(self) -> &'static str {
        match self {
            NamedPrimaries::Srgb => "srgb",
            NamedPrimaries::PalM => "pal_m",
            NamedPrimaries::Pal => "pal",
            NamedPrimaries::Ntsc => "ntsc",
            NamedPrimaries::GenericFilm => "generic_film",
            NamedPrimaries::Bt2020 => "bt2020",
            NamedPrimaries::Cie1931Xyz => "cie1931_xyz",
            NamedPrimaries::DciP3 => "dci_p3",
            NamedPrimaries::DisplayP3 => "display_p3",
            NamedPrimaries::AdobeRgb => "adobe_rgb",
        }
    }

    /// Its chromaticities, as the standards that name it give them (the same values as KWin's
    /// `colorspace.cpp`, read on 2026-11-14).
    pub fn chromaticities(self) -> Chromaticities {
        const D65: (i32, i32) = (312_700, 329_000);
        const C: (i32, i32) = (310_000, 316_000);
        let (red, green, blue, white) = match self {
            NamedPrimaries::Srgb => (
                (640_000, 330_000),
                (300_000, 600_000),
                (150_000, 60_000),
                D65,
            ),
            NamedPrimaries::PalM => ((670_000, 330_000), (210_000, 710_000), (140_000, 80_000), C),
            NamedPrimaries::Pal => (
                (640_000, 330_000),
                (290_000, 600_000),
                (150_000, 60_000),
                D65,
            ),
            NamedPrimaries::Ntsc => (
                (630_000, 340_000),
                (310_000, 595_000),
                (155_000, 70_000),
                D65,
            ),
            NamedPrimaries::GenericFilm => {
                ((681_000, 319_000), (243_000, 692_000), (145_000, 49_000), C)
            }
            NamedPrimaries::Bt2020 => (
                (708_000, 292_000),
                (170_000, 797_000),
                (131_000, 46_000),
                D65,
            ),
            NamedPrimaries::Cie1931Xyz => {
                ((1_000_000, 0), (0, 1_000_000), (0, 0), (333_333, 333_333))
            }
            NamedPrimaries::DciP3 => (
                (680_000, 320_000),
                (265_000, 690_000),
                (150_000, 60_000),
                (314_000, 351_000),
            ),
            NamedPrimaries::DisplayP3 => (
                (680_000, 320_000),
                (265_000, 690_000),
                (150_000, 60_000),
                D65,
            ),
            NamedPrimaries::AdobeRgb => (
                (640_000, 330_000),
                (210_000, 710_000),
                (150_000, 60_000),
                D65,
            ),
        };
        Chromaticities {
            red,
            green,
            blue,
            white,
        }
    }
}

/// CIE 1931 xy chromaticities of three primaries and a white point, in millionths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chromaticities {
    /// The red primary's x and y.
    pub red: (i32, i32),
    /// The green primary's x and y.
    pub green: (i32, i32),
    /// The blue primary's x and y.
    pub blue: (i32, i32),
    /// The white point's x and y.
    pub white: (i32, i32),
}

impl Chromaticities {
    /// Whether they can describe colours at all: three primaries that are not on one line, and
    /// a white point with a luminance (y above 0). A blue at y = 0, CIE XYZ's, is allowed.
    pub fn is_usable(&self) -> bool {
        // In 128 bits: a difference of two `i32` takes 33 bits, a product of two 66.
        let (r, g, b) = (self.red, self.green, self.blue);
        let d = |a: i32, b: i32| i128::from(a) - i128::from(b);
        let twice_area = d(g.0, r.0) * d(b.1, r.1) - d(b.0, r.0) * d(g.1, r.1);
        twice_area != 0 && self.white.1 > 0
    }

    /// The eight values, in the protocol's order: red, green, blue, white, x before y.
    pub fn values(&self) -> [i32; 8] {
        [
            self.red.0,
            self.red.1,
            self.green.0,
            self.green.1,
            self.blue.0,
            self.blue.1,
            self.white.0,
            self.white.1,
        ]
    }

    /// From the eight values, in the protocol's order.
    pub fn from_values(v: [i32; 8]) -> Chromaticities {
        Chromaticities {
            red: (v[0], v[1]),
            green: (v[2], v[3]),
            blue: (v[4], v[5]),
            white: (v[6], v[7]),
        }
    }
}

/// A colour volume's luminances: its minimum in ten-thousandths of a cd/m², its maximum and
/// its reference white in cd/m².
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Luminances {
    /// The minimum, in ten-thousandths of a cd/m².
    pub min: u32,
    /// The maximum, in cd/m².
    pub max: u32,
    /// The reference white, in cd/m².
    pub reference: u32,
}

impl Luminances {
    /// The protocol's default, sRGB's: 0.2, 80 and 80 cd/m².
    pub const SRGB: Luminances = Luminances {
        min: 2_000,
        max: 80,
        reference: 80,
    };
}

/// An image description: what a surface's pixel values mean, or what an output expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Description {
    /// How pixel values encode light.
    pub transfer_function: TransferFunction,
    /// The primaries and white point colours are encoded in.
    pub primaries: Chromaticities,
    /// The name of `primaries`, when they were given by name.
    pub named_primaries: Option<NamedPrimaries>,
    /// The luminances of the primaries' volume.
    pub luminances: Luminances,
    /// The target (mastering) colour volume: its primaries, then its minimum (ten-thousandths of
    /// a cd/m²) and maximum (cd/m²) luminance.
    pub target_primaries: Chromaticities,
    /// The target's minimum and maximum luminance.
    pub target_luminance: (u32, u32),
    /// CTA-861-H's maximum content light level, in cd/m², when given.
    pub max_cll: Option<u32>,
    /// CTA-861-H's maximum frame-average light level, in cd/m², when given.
    pub max_fall: Option<u32>,
}

/// Why a parameter of an image description was refused: each is the protocol error of
/// `wp_image_description_creator_params_v1` that ends the client, except `Unusable`, which fails
/// the description gracefully.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamsError {
    /// The transfer function or the primaries are missing at `create`.
    IncompleteSet,
    /// A property set twice.
    AlreadySet,
    /// A request of a feature Yuna does not list.
    UnsupportedFeature,
    /// A transfer function Yuna does not list.
    InvalidTf,
    /// Primaries the protocol does not name.
    InvalidPrimariesNamed,
    /// A luminance not above its minimum, or a frame average above the content light level.
    InvalidLuminance,
    /// Primaries that describe no colour: the `failed` event, cause `unsupported`.
    Unusable,
}

impl fmt::Display for ParamsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ParamsError::IncompleteSet => "the transfer function or the primaries are not set",
            ParamsError::AlreadySet => "this property is set already",
            ParamsError::UnsupportedFeature => "Yuna does not list this feature",
            ParamsError::InvalidTf => "Yuna does not list this transfer function",
            ParamsError::InvalidPrimariesNamed => "these are not primaries the protocol names",
            ParamsError::InvalidLuminance => "a luminance is not above the minimum",
            ParamsError::Unusable => "the primaries are on one line or the white has no luminance",
        })
    }
}

/// The parameters a client sets before it creates a description, each once
/// (`wp_image_description_creator_params_v1`).
#[derive(Debug, Default)]
pub struct Params {
    transfer_function: Option<TransferFunction>,
    primaries: Option<(Chromaticities, Option<NamedPrimaries>)>,
    luminances: Option<Luminances>,
    target_primaries: Option<Chromaticities>,
    target_luminance: Option<(u32, u32)>,
    max_cll: Option<u32>,
    max_fall: Option<u32>,
}

impl Params {
    /// `set_tf_named`, at the client's `version`.
    pub fn set_transfer_function(&mut self, value: u32, version: u32) -> Result<(), ParamsError> {
        if self.transfer_function.is_some() {
            return Err(ParamsError::AlreadySet);
        }
        let tf = TransferFunction::from_wire(value, version).ok_or(ParamsError::InvalidTf)?;
        self.transfer_function = Some(tf);
        Ok(())
    }

    /// `set_primaries_named`.
    pub fn set_named_primaries(&mut self, value: u32) -> Result<(), ParamsError> {
        if self.primaries.is_some() {
            return Err(ParamsError::AlreadySet);
        }
        let named = NamedPrimaries::from_wire(value).ok_or(ParamsError::InvalidPrimariesNamed)?;
        self.primaries = Some((named.chromaticities(), Some(named)));
        Ok(())
    }

    /// `set_primaries`: any values; ones that describe no colour fail the description when it is
    /// created, gracefully.
    pub fn set_primaries(&mut self, primaries: Chromaticities) -> Result<(), ParamsError> {
        if self.primaries.is_some() {
            return Err(ParamsError::AlreadySet);
        }
        self.primaries = Some((primaries, None));
        Ok(())
    }

    /// `set_luminances`: the maximum and the reference must be above the minimum, compared in
    /// the same unit. With PQ, the maximum is then replaced by the minimum plus 10,000 cd/m².
    pub fn set_luminances(&mut self, luminances: Luminances) -> Result<(), ParamsError> {
        if self.luminances.is_some() {
            return Err(ParamsError::AlreadySet);
        }
        let min = u64::from(luminances.min);
        if u64::from(luminances.max) * 10_000 <= min
            || u64::from(luminances.reference) * 10_000 <= min
        {
            return Err(ParamsError::InvalidLuminance);
        }
        self.luminances = Some(luminances);
        Ok(())
    }

    /// `set_mastering_display_primaries`. Primaries that describe no colour are dropped, as
    /// KWin drops them: a refusal would end a Mesa client (ADR-0047). All eight at 0 means
    /// unset.
    pub fn set_target_primaries(&mut self, primaries: Chromaticities) -> Result<(), ParamsError> {
        if self.target_primaries.is_some() {
            return Err(ParamsError::AlreadySet);
        }
        // Set, even when dropped, so that a second request is still `already_set`.
        self.target_primaries = Some(primaries);
        Ok(())
    }

    /// `set_mastering_luminance`: a maximum of 0 means unset, as Mesa and mpv send it; any
    /// other must be above the minimum.
    pub fn set_target_luminance(&mut self, min: u32, max: u32) -> Result<(), ParamsError> {
        if self.target_luminance.is_some() {
            return Err(ParamsError::AlreadySet);
        }
        if max != 0 && u64::from(max) * 10_000 <= u64::from(min) {
            return Err(ParamsError::InvalidLuminance);
        }
        self.target_luminance = Some((min, max));
        Ok(())
    }

    /// `set_max_cll`: 0 means unset. The protocol names no `already_set` here: the last wins.
    pub fn set_max_cll(&mut self, value: u32) {
        self.max_cll = Some(value).filter(|v| *v != 0);
    }

    /// `set_max_fall`: 0 means unset; the last wins.
    pub fn set_max_fall(&mut self, value: u32) {
        self.max_fall = Some(value).filter(|v| *v != 0);
    }

    /// `create`: the description, with each default filled in. Version 1's further checks of
    /// `max_cll` and `max_fall` against the target range are not made, as in version 2: Mesa
    /// binds version 1 and sends them unchecked (ADR-0047).
    pub fn create(self) -> Result<Description, ParamsError> {
        let (Some(transfer_function), Some((primaries, named_primaries))) =
            (self.transfer_function, self.primaries)
        else {
            return Err(ParamsError::IncompleteSet);
        };
        if let (Some(cll), Some(fall)) = (self.max_cll, self.max_fall)
            && fall > cll
        {
            return Err(ParamsError::InvalidLuminance);
        }
        if !primaries.is_usable() {
            return Err(ParamsError::Unusable);
        }
        let mut luminances = self
            .luminances
            .unwrap_or_else(|| transfer_function.default_luminances());
        if transfer_function == TransferFunction::St2084Pq {
            // PQ's swing is 10,000 cd/m² above its minimum, whatever the client sent.
            let max = u64::from(luminances.min) / 10_000 + 10_000;
            luminances.max = u32::try_from(max).unwrap_or(u32::MAX);
        }
        let target_primaries = self
            .target_primaries
            .filter(Chromaticities::is_usable)
            .unwrap_or(primaries);
        let target_luminance = self
            .target_luminance
            .filter(|(_, max)| *max != 0)
            .unwrap_or((luminances.min, luminances.max));
        Ok(Description {
            transfer_function,
            primaries,
            named_primaries,
            luminances,
            target_primaries,
            target_luminance,
            max_cll: self.max_cll,
            max_fall: self.max_fall,
        })
    }
}

/// A monitor whose colours an HDR virtual output announces
/// ([ADR-0048](../../../../docs/adr/0048-hdr-virtual-outputs-pq-announced-drawn-at-stage-5.md)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Monitor {
    /// The name `outputs list` shows.
    pub name: &'static str,
    /// The panel's primaries and white point.
    pub primaries: Chromaticities,
    /// Its desired content minimum luminance, in ten-thousandths of a cd/m².
    pub min_luminance: u32,
    /// Its desired content maximum luminance, in cd/m².
    pub max_luminance: u32,
    /// Its desired content maximum frame-average luminance, in cd/m².
    pub max_frame_average: u32,
}

/// The owner's monitor, an ASUS PG32UCDM, from its EDID read on 2026-11-14 with edid-decode
/// 1.32: its colour characteristics, and its HDR static metadata (desired content luminance
/// 1015.2 cd/m² at most, 265.0 on a frame's average, code 1 at least, 0.000156 cd/m², which the
/// protocol's ten-thousandths round to 0.0002).
pub const PG32UCDM: Monitor = Monitor {
    name: "PG32UCDM",
    primaries: Chromaticities {
        red: (683_500, 304_600),
        green: (244_100, 708_900),
        blue: (143_500, 55_600),
        white: (313_400, 329_100),
    },
    min_luminance: 2,
    max_luminance: 1_015,
    max_frame_average: 265,
};

impl Monitor {
    /// What an HDR output that shows it announces: PQ over BT.2020, a reference white of 203
    /// cd/m², and the monitor's own volume as the target. The transfer function is provisional
    /// until Stage 5 chooses its pipeline (ADR-0048).
    pub fn hdr_description(&self) -> Description {
        Description {
            transfer_function: TransferFunction::St2084Pq,
            primaries: NamedPrimaries::Bt2020.chromaticities(),
            named_primaries: Some(NamedPrimaries::Bt2020),
            luminances: TransferFunction::St2084Pq.default_luminances(),
            target_primaries: self.primaries,
            target_luminance: (self.min_luminance, self.max_luminance),
            max_cll: Some(self.max_luminance),
            max_fall: Some(self.max_frame_average),
        }
    }
}

/// What an SDR output announces: gamma 2.2, sRGB's primaries and luminances (ADR-0048).
pub fn sdr_description() -> Description {
    let primaries = NamedPrimaries::Srgb.chromaticities();
    let luminances = Luminances::SRGB;
    Description {
        transfer_function: TransferFunction::Gamma22,
        primaries,
        named_primaries: Some(NamedPrimaries::Srgb),
        luminances,
        target_primaries: primaries,
        target_luminance: (luminances.min, luminances.max),
        max_cll: None,
        max_fall: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pq_bt2020() -> Params {
        let mut params = Params::default();
        params.set_transfer_function(11, 1).expect("PQ");
        params.set_named_primaries(6).expect("BT.2020");
        params
    }

    #[test]
    fn a_complete_set_takes_its_transfer_functions_defaults() {
        let description = pq_bt2020().create().expect("a description");
        assert_eq!(
            description.luminances,
            Luminances {
                min: 50,
                max: 10_000,
                reference: 203
            }
        );
        assert_eq!(description.target_primaries, description.primaries);
        assert_eq!(description.target_luminance, (50, 10_000));
        assert_eq!(description.max_cll, None);
    }

    #[test]
    fn an_incomplete_set_is_refused() {
        let mut params = Params::default();
        params.set_transfer_function(2, 1).expect("gamma 2.2");
        assert_eq!(params.create(), Err(ParamsError::IncompleteSet));
        let mut params = Params::default();
        params.set_named_primaries(1).expect("sRGB");
        assert_eq!(params.create(), Err(ParamsError::IncompleteSet));
    }

    #[test]
    fn each_property_is_set_once() {
        let mut params = pq_bt2020();
        assert_eq!(
            params.set_transfer_function(2, 1),
            Err(ParamsError::AlreadySet)
        );
        assert_eq!(params.set_named_primaries(1), Err(ParamsError::AlreadySet));
        assert_eq!(
            params.set_primaries(NamedPrimaries::Srgb.chromaticities()),
            Err(ParamsError::AlreadySet)
        );
        params.set_luminances(Luminances::SRGB).expect("once");
        assert_eq!(
            params.set_luminances(Luminances::SRGB),
            Err(ParamsError::AlreadySet)
        );
        params.set_target_luminance(1, 1_000).expect("once");
        assert_eq!(
            params.set_target_luminance(1, 1_000),
            Err(ParamsError::AlreadySet)
        );
        params
            .set_target_primaries(NamedPrimaries::DisplayP3.chromaticities())
            .expect("once");
        assert_eq!(
            params.set_target_primaries(NamedPrimaries::DisplayP3.chromaticities()),
            Err(ParamsError::AlreadySet)
        );
    }

    #[test]
    fn transfer_functions_yuna_does_not_list_are_refused() {
        let mut params = Params::default();
        // srgb (9), gamma28 (3), unknown (0, 99).
        for value in [9, 3, 0, 99] {
            assert_eq!(
                params.set_transfer_function(value, 3),
                Err(ParamsError::InvalidTf)
            );
        }
        // compound_power_2_4 only from version 2.
        assert_eq!(
            params.set_transfer_function(14, 1),
            Err(ParamsError::InvalidTf)
        );
        params.set_transfer_function(14, 2).expect("version 2");
    }

    #[test]
    fn unknown_named_primaries_are_refused() {
        let mut params = Params::default();
        for value in [0, 11, u32::MAX] {
            assert_eq!(
                params.set_named_primaries(value),
                Err(ParamsError::InvalidPrimariesNamed)
            );
        }
        for named in NamedPrimaries::ALL {
            assert_eq!(NamedPrimaries::from_wire(named.wire()), Some(named));
        }
    }

    #[test]
    fn luminances_at_or_below_the_minimum_are_refused_without_overflow() {
        let mut params = pq_bt2020();
        // 80 cd/m² against a minimum of 80 cd/m².
        let at_minimum = Luminances {
            min: 800_000,
            max: 80,
            reference: 100,
        };
        assert_eq!(
            params.set_luminances(at_minimum),
            Err(ParamsError::InvalidLuminance)
        );
        let extreme = Luminances {
            min: u32::MAX,
            max: u32::MAX,
            reference: u32::MAX,
        };
        params
            .set_luminances(extreme)
            .expect("in 64 bits, above the minimum");
        // PQ's maximum is its minimum plus 10,000 cd/m², computed without overflow.
        let description = params.create().expect("a description");
        assert_eq!(description.luminances.max, u32::MAX / 10_000 + 10_000);
    }

    #[test]
    fn zeros_mean_unset_as_mesa_and_mpv_send_them() {
        let mut params = pq_bt2020();
        params.set_max_cll(0);
        params.set_max_fall(0);
        params.set_target_luminance(0, 0).expect("unset");
        params
            .set_target_primaries(Chromaticities::from_values([0; 8]))
            .expect("unset");
        let description = params.create().expect("a description");
        assert_eq!(description.max_cll, None);
        assert_eq!(description.max_fall, None);
        assert_eq!(description.target_luminance, (50, 10_000));
        assert_eq!(description.target_primaries, description.primaries);
    }

    #[test]
    fn a_frame_average_above_the_content_light_level_is_refused() {
        let mut params = pq_bt2020();
        params.set_max_cll(400);
        params.set_max_fall(500);
        assert_eq!(params.create(), Err(ParamsError::InvalidLuminance));
    }

    #[test]
    fn primaries_on_one_line_fail_gracefully() {
        let mut params = Params::default();
        params.set_transfer_function(2, 1).expect("gamma 2.2");
        params
            .set_primaries(Chromaticities::from_values([
                100_000, 100_000, 200_000, 200_000, 300_000, 300_000, 312_700, 329_000,
            ]))
            .expect("set");
        assert_eq!(params.create(), Err(ParamsError::Unusable));
    }

    #[test]
    fn extreme_primaries_are_judged_without_overflow() {
        let extreme = Chromaticities::from_values([
            i32::MIN,
            i32::MAX,
            i32::MAX,
            i32::MIN,
            i32::MIN,
            i32::MIN,
            i32::MAX,
            i32::MAX,
        ]);
        assert!(extreme.is_usable());
        let mut params = Params::default();
        params.set_transfer_function(2, 1).expect("gamma 2.2");
        params.set_primaries(extreme).expect("set");
        params.set_target_primaries(extreme).expect("set");
        assert!(params.create().is_ok());
    }

    #[test]
    fn every_named_set_is_usable() {
        for named in NamedPrimaries::ALL {
            assert!(named.chromaticities().is_usable(), "{named:?}");
        }
    }

    #[test]
    fn the_monitors_description_is_its_edids_volume_over_pq() {
        let description = PG32UCDM.hdr_description();
        assert_eq!(description.transfer_function, TransferFunction::St2084Pq);
        assert_eq!(description.named_primaries, Some(NamedPrimaries::Bt2020));
        assert_eq!(description.luminances.reference, 203);
        assert_eq!(description.target_luminance, (2, 1_015));
        assert_eq!(description.max_cll, Some(1_015));
        assert_eq!(description.max_fall, Some(265));
        assert_eq!(description.target_primaries.red, (683_500, 304_600));
    }
}
