//! Where the snow line sits (`--snow-mode`): the climatic line for the
//! latitude, the top share of the area's relief, a fixed Y, or nowhere.
//!
//! Every mode resolves to the one Y threshold the snow cover, the mountain
//! biomes and the alpine band already read, so they stay in step.

/// Least relief, in metres, that `peaks` caps: on gentler ground the top few
/// percent is a field or a hilltop, and a white speckle there reads as a bug.
const PEAKS_MIN_RELIEF_M: f64 = 150.0;

#[derive(clap::ValueEnum, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SnowMode {
    /// The climatic snow line for the latitude.
    #[default]
    Realistic,
    /// The top --snow-percent of the area's height range.
    Peaks,
    /// From --snow-y up.
    Manual,
    /// No snowfall. Mountain biomes and glaciers keep to the climatic line.
    Off,
}

/// Snow line options. Off unless asked for, so the GUI runs on `SnowArgs::default()`.
#[derive(clap::Args, Debug, Default)]
#[command(next_help_heading = "Snow")]
pub struct SnowArgs {
    /// Where snow lies on terrain: realistic (the climatic snow line, default),
    /// peaks (the top --snow-percent of the area's height range, none under
    /// 150 m of relief), manual (from --snow-y up) or off.
    #[arg(long, value_enum, default_value_t = SnowMode::Realistic)]
    pub snow_mode: SnowMode,

    /// Share of the height range that --snow-mode peaks covers, 0 to 100 (default 6).
    #[arg(long, value_name = "PERCENT", value_parser = parse_percent)]
    pub snow_percent: Option<f64>,

    /// Minecraft Y from which --snow-mode manual lays snow.
    #[arg(long, value_name = "Y", allow_hyphen_values = true)]
    pub snow_y: Option<i32>,
}

impl SnowArgs {
    pub const DEFAULT_PERCENT: f64 = 6.0;

    /// Refuses options that would be silently ignored, and `peaks` in One
    /// World, where each area or piece would cap its own relief and the snow
    /// line would step at every seam.
    pub fn validate(&self, one_world: bool) -> Result<(), String> {
        if self.snow_percent.is_some() && self.snow_mode != SnowMode::Peaks {
            return Err("--snow-percent only applies to --snow-mode peaks.".to_string());
        }
        match (self.snow_mode, self.snow_y) {
            (SnowMode::Manual, None) => Err("--snow-mode manual needs --snow-y.".to_string()),
            (SnowMode::Manual, Some(_)) | (_, None) => Ok(()),
            _ => Err("--snow-y only applies to --snow-mode manual.".to_string()),
        }?;
        if one_world && self.snow_mode == SnowMode::Peaks {
            return Err(
                "--snow-mode peaks follows each area's own relief, so One World areas would not meet; use --snow-mode manual --snow-y instead."
                    .to_string(),
            );
        }
        Ok(())
    }

    /// Snow line Y for a run whose climatic line is `realistic_y`, whose
    /// terrain spans `relief_y` (lowest, highest) at `blocks_per_meter`.
    pub fn threshold_y(
        &self,
        realistic_y: i32,
        relief_y: Option<(f64, f64)>,
        blocks_per_meter: f64,
    ) -> i32 {
        match self.snow_mode {
            SnowMode::Realistic | SnowMode::Off => realistic_y,
            SnowMode::Manual => self.snow_y.unwrap_or(realistic_y),
            SnowMode::Peaks => relief_y.map_or(i32::MAX, |(lo, hi)| {
                let percent = self.snow_percent.unwrap_or(Self::DEFAULT_PERCENT);
                peaks_y(lo, hi, blocks_per_meter, percent)
            }),
        }
    }

    /// Whether snow is laid at all.
    pub fn snowfall(&self) -> bool {
        self.snow_mode != SnowMode::Off
    }
}

/// Y from which the top `percent` of `lo..=hi` is snow, or `i32::MAX` when
/// the relief is too low to have peaks.
fn peaks_y(lo: f64, hi: f64, blocks_per_meter: f64, percent: f64) -> i32 {
    if blocks_per_meter <= 0.0 || (hi - lo) / blocks_per_meter < PEAKS_MIN_RELIEF_M {
        return i32::MAX;
    }
    (hi - (hi - lo) * percent / 100.0).round() as i32
}

fn parse_percent(arg: &str) -> Result<f64, String> {
    match arg.parse::<f64>() {
        Ok(p) if (0.0..=100.0).contains(&p) => Ok(p),
        _ => Err(format!("`{arg}` is not a percentage from 0 to 100")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        snow: SnowArgs,
    }

    fn parse(argv: &[&str]) -> Result<SnowArgs, String> {
        let cli = Cli::try_parse_from(std::iter::once("arnis").chain(argv.iter().copied()))
            .map_err(|e| e.to_string())?;
        cli.snow.validate(false)?;
        Ok(cli.snow)
    }

    #[test]
    fn the_default_is_the_climatic_line() {
        let snow = parse(&[]).unwrap();
        assert_eq!(snow.snow_mode, SnowMode::Realistic);
        assert_eq!(snow.threshold_y(321, Some((0.0, 900.0)), 1.0), 321);
        assert_eq!(snow.threshold_y(i32::MAX, None, 1.0), i32::MAX);
        assert!(snow.snowfall());
    }

    #[test]
    fn off_keeps_the_line_but_lays_no_snow() {
        let snow = parse(&["--snow-mode", "off"]).unwrap();
        assert_eq!(snow.threshold_y(321, Some((0.0, 900.0)), 1.0), 321);
        assert!(!snow.snowfall());
    }

    #[test]
    fn manual_puts_the_line_at_the_given_y() {
        let snow = parse(&["--snow-mode", "manual", "--snow-y", "-20"]).unwrap();
        assert_eq!(snow.threshold_y(321, Some((0.0, 900.0)), 1.0), -20);
    }

    #[test]
    fn peaks_caps_the_top_share_of_the_relief() {
        let snow = parse(&["--snow-mode", "peaks", "--snow-percent", "10"]).unwrap();
        // 1000 m of relief at 0.1 block/m from Y 0: the top 10 blocks.
        assert_eq!(snow.threshold_y(i32::MAX, Some((0.0, 100.0)), 0.1), 90);
        let default = parse(&["--snow-mode", "peaks"]).unwrap();
        assert_eq!(default.threshold_y(0, Some((-50.0, 450.0)), 1.0), 420);
        // Under 150 m of relief, and without terrain, nothing is a peak.
        assert_eq!(snow.threshold_y(0, Some((0.0, 140.0)), 1.0), i32::MAX);
        assert_eq!(snow.threshold_y(0, None, 1.0), i32::MAX);
    }

    #[test]
    fn options_that_would_be_ignored_are_refused() {
        assert!(parse(&["--snow-mode", "manual"]).is_err());
        assert!(parse(&["--snow-y", "200"]).is_err());
        assert!(parse(&["--snow-mode", "peaks", "--snow-y", "200"]).is_err());
        assert!(parse(&["--snow-percent", "5"]).is_err());
        assert!(parse(&["--snow-mode", "peaks", "--snow-percent", "101"]).is_err());
        assert!(parse(&["--snow-mode", "peaks", "--snow-percent", "-1"]).is_err());
    }

    #[test]
    fn peaks_is_refused_in_one_world_and_manual_is_not() {
        let peaks = parse(&["--snow-mode", "peaks"]).unwrap();
        assert!(peaks.validate(true).is_err());
        let manual = parse(&["--snow-mode", "manual", "--snow-y", "200"]).unwrap();
        assert!(manual.validate(true).is_ok());
    }
}
