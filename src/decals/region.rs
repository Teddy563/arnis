//! Sign style picked from the bbox centre (One World: the world origin): blade colour, speed sign
//! shape, metro logo.
//! Falls back to the continental European look.

use super::registry::SpeedStyle;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SignRegion {
    /// Continental Europe and everywhere not matched below.
    Europe,
    /// Germany, Austria, Switzerland: blue street blades, "U" metro logo.
    Germanic,
    /// UK and Ireland: white blades with black text, mph.
    UkIreland,
    /// USA: green blades, "SPEED LIMIT" plates, mph.
    NorthAmerica,
    /// Canada: green blades, "MAXIMUM" plates, km/h.
    Canada,
    /// Australia and New Zealand: green blades, metric.
    Oceania,
    /// Japan: metric, "M" logo, blue blades.
    Japan,
}

impl SignRegion {
    /// The region a run paints and signs for: its bbox centre, or in One World the
    /// world's origin, so every piece of a world picks the same one.
    pub fn for_run(
        args: &crate::args::Args,
        llbbox: &crate::coordinate_system::geographic::LLBBox,
    ) -> SignRegion {
        match &args.one_world_run {
            Some(run) => SignRegion::detect(run.origin_lat, run.origin_lon),
            None => SignRegion::detect(
                (llbbox.min().lat() + llbbox.max().lat()) / 2.0,
                (llbbox.min().lng() + llbbox.max().lng()) / 2.0,
            ),
        }
    }

    /// Picks the region for a lat/lon (degrees).
    pub fn detect(lat: f64, lon: f64) -> SignRegion {
        if (15.0..85.0).contains(&lat) && (-170.0..-50.0).contains(&lon) {
            // Canada: north of the 49th parallel, plus southern Ontario/Quebec and the
            // Maritimes below it. Rough boxes; a few border towns land on the wrong side.
            let canada = lat >= 49.0
                || (lat > 44.9 && (-84.0..-71.0).contains(&lon))
                || ((43.0..44.9).contains(&lat) && (-80.0..-78.0).contains(&lon))
                || (lat > 44.5 && (-66.0..-52.0).contains(&lon));
            return if canada {
                SignRegion::Canada
            } else {
                SignRegion::NorthAmerica
            };
        }
        if (-48.0..-9.0).contains(&lat) && (110.0..180.0).contains(&lon) {
            return SignRegion::Oceania;
        }
        if (30.0..46.0).contains(&lat) && (128.0..146.5).contains(&lon) {
            return SignRegion::Japan;
        }
        if (49.9..61.0).contains(&lat) && (-11.0..1.8).contains(&lon) {
            return SignRegion::UkIreland;
        }
        if (51.5..55.1).contains(&lat) && (-11.0..-5.3).contains(&lon) {
            return SignRegion::UkIreland;
        }
        // Germany, Austria and Switzerland, roughly; the Alps box also catches Liechtenstein.
        if (47.2..55.1).contains(&lat) && (5.9..15.1).contains(&lon) {
            return SignRegion::Germanic;
        }
        if (45.8..47.3).contains(&lat) && (5.9..10.5).contains(&lon) {
            return SignRegion::Germanic;
        }
        if (46.3..49.0).contains(&lat) && (10.5..17.2).contains(&lon) {
            return SignRegion::Germanic;
        }
        SignRegion::Europe
    }

    /// Street name blade colour family.
    pub fn blade_style(self) -> BladeStyle {
        match self {
            SignRegion::Germanic | SignRegion::Europe | SignRegion::Japan => BladeStyle::Blue,
            SignRegion::UkIreland => BladeStyle::White,
            SignRegion::NorthAmerica | SignRegion::Canada | SignRegion::Oceania => {
                BladeStyle::Green
            }
        }
    }

    /// Shape of the speed limit sign.
    pub fn speed_style(self) -> SpeedStyle {
        match self {
            SignRegion::NorthAmerica => SpeedStyle::UsPlate,
            SignRegion::Canada => SpeedStyle::CaPlate,
            _ => SpeedStyle::Disc,
        }
    }

    /// Whether unlabelled `maxspeed` values are miles per hour.
    pub fn default_mph(self) -> bool {
        matches!(self, SignRegion::UkIreland | SignRegion::NorthAmerica)
    }

    /// Whether traffic keeps left, which puts roadside signs on the left kerb.
    pub fn drives_on_left(self) -> bool {
        matches!(
            self,
            SignRegion::UkIreland | SignRegion::Oceania | SignRegion::Japan
        )
    }

    /// Pictogram for `railway=subway_entrance`.
    pub fn metro_logo(self) -> &'static str {
        match self {
            SignRegion::Germanic => "metro_u",
            _ => "metro_m",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BladeStyle {
    Blue,
    Green,
    White,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::geographic::LLBBox;

    #[test]
    fn detects_major_regions() {
        assert_eq!(SignRegion::detect(52.52, 13.40), SignRegion::Germanic); // Berlin
        assert_eq!(SignRegion::detect(48.20, 16.37), SignRegion::Germanic); // Vienna
        assert_eq!(SignRegion::detect(47.37, 8.54), SignRegion::Germanic); // Zurich
        assert_eq!(SignRegion::detect(48.85, 2.35), SignRegion::Europe); // Paris
        assert_eq!(SignRegion::detect(51.50, -0.12), SignRegion::UkIreland); // London
        assert_eq!(SignRegion::detect(53.35, -6.26), SignRegion::UkIreland); // Dublin
        assert_eq!(SignRegion::detect(40.71, -74.0), SignRegion::NorthAmerica); // NYC
        assert_eq!(SignRegion::detect(43.65, -79.38), SignRegion::Canada); // Toronto
        assert_eq!(SignRegion::detect(45.50, -73.57), SignRegion::Canada); // Montreal
        assert_eq!(SignRegion::detect(49.28, -123.12), SignRegion::Canada); // Vancouver
        assert_eq!(SignRegion::detect(44.65, -63.57), SignRegion::Canada); // Halifax
        assert_eq!(SignRegion::detect(47.61, -122.33), SignRegion::NorthAmerica); // Seattle
        assert_eq!(SignRegion::detect(42.36, -71.06), SignRegion::NorthAmerica); // Boston
        assert!(SignRegion::detect(51.50, -0.12).drives_on_left());
        assert!(!SignRegion::detect(52.52, 13.40).drives_on_left());
        assert_eq!(SignRegion::detect(-33.87, 151.2), SignRegion::Oceania); // Sydney
        assert_eq!(SignRegion::detect(35.68, 139.69), SignRegion::Japan); // Tokyo
        assert_eq!(SignRegion::detect(50.95, 1.85), SignRegion::Europe); // Calais
        assert_eq!(SignRegion::detect(52.37, 4.90), SignRegion::Europe); // Amsterdam
    }

    #[test]
    fn one_world_pieces_take_the_world_origins_region() {
        use clap::Parser;
        // Two pieces of one world either side of the Germanic box's west edge (5.9 E).
        let west = LLBBox::from_str("48.0,5.7,48.1,5.8").unwrap();
        let east = LLBBox::from_str("48.0,6.0,48.1,6.1").unwrap();
        let mut args = crate::args::Args::parse_from(["arnis", "--bbox", "48.0,5.7,48.1,5.8"]);
        assert_eq!(SignRegion::for_run(&args, &west), SignRegion::Europe);
        assert_eq!(SignRegion::for_run(&args, &east), SignRegion::Germanic);
        args.one_world_run = Some(crate::one_world::RunContext {
            world_dir: std::path::PathBuf::new(),
            origin_lat: 48.05,
            origin_lon: 6.0,
            extending: false,
            elevation: None,
            elevation_zoom: None,
            replaced_chunks: 0,
            area_id: 0,
            unit: None,
        });
        assert_eq!(SignRegion::for_run(&args, &west), SignRegion::Germanic);
        assert_eq!(SignRegion::for_run(&args, &east), SignRegion::Germanic);
    }
}
