//! `--road-detail`: how much paint and how many minor ways a road network gets.
//!
//! At full detail every lane divider, zebra bar and footway is drawn. Below
//! about 0.7 blocks per metre those features land on the same few blocks as
//! the carriageway and read as a white checker at junctions. `clean` keeps
//! every way but simplifies the markings; `compact` also drops the minor
//! ways. Every decision is a pure function of tags and world position, so a
//! One World piece boundary never changes the result.

use crate::osm_parser::ProcessedElement;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum RoadDetail {
    /// Every highway and marking, as mapped
    #[default]
    Max,
    /// At most a centre stripe (a double one from 5 lanes), none on service
    /// and pedestrian ways or roads under 4 blocks wide, 2-block zebra bars
    Clean,
    /// Clean markings with longer dashes, and no footways, paths, cycleways,
    /// steps, service roads, tracks or crossings
    Compact,
}

/// Ways for people rather than traffic. They never get lane dividers outside
/// `max`, and `compact` leaves them out together with service roads and tracks.
fn is_pedestrian_grade(highway: &str) -> bool {
    matches!(
        highway,
        "footway"
            | "path"
            | "cycleway"
            | "steps"
            | "corridor"
            | "pedestrian"
            | "platform"
            | "bus_stop"
            | "track"
    )
}

impl RoadDetail {
    /// Whether `compact` leaves this highway out: minor ways and every
    /// crossing marker, which sit on top of the carriageway.
    pub fn skips_highway(self, tags: &HashMap<String, String>) -> bool {
        let Some(highway) = tags.get("highway") else {
            return false;
        };
        self == RoadDetail::Compact
            && (is_pedestrian_grade(highway)
                || matches!(highway.as_str(), "service" | "crossing")
                || tags.contains_key("crossing")
                || tags.get("footway").map(String::as_str) == Some("crossing"))
    }

    /// Whether to drop the element before generation. Only elements that would
    /// reach the highway generator go: one that also carries a building,
    /// amenity, barrier and so on is still built by that handler. Dropping them
    /// up front keeps the road mask, junctions and signage in step with the
    /// roads actually drawn.
    pub fn drops_element(self, element: &ProcessedElement) -> bool {
        let tags = element.tags();
        !matches!(element, ProcessedElement::Relation(_))
            && self.skips_highway(tags)
            && ![
                "building",
                "building:part",
                "aeroway",
                "amenity",
                "barrier",
                "door",
                "entrance",
                "natural",
            ]
            .iter()
            .any(|key| tags.contains_key(*key))
    }

    /// Dash and gap length of a lane divider in blocks. `compact` keeps them
    /// at least 4 long so short dashes do not merge into a checker.
    pub fn dash_length(self, scale: f64) -> i32 {
        let dash = (5.0 * scale).ceil() as i32;
        if self == RoadDetail::Compact {
            dash.max(4)
        } else {
            dash
        }
    }

    /// Lane count that sets the dividers (`lanes - 1` of them) and whether the
    /// two dividers become a double centre line. `lanes` is the mapped count
    /// after `lane_markings=no`, `road_width` the full width in blocks.
    pub fn lane_plan(
        self,
        highway: &str,
        tags: &HashMap<String, String>,
        lanes: i32,
        road_width: i32,
    ) -> (i32, bool) {
        if self == RoadDetail::Max {
            return (lanes, false);
        }
        // A stripe on a road under 4 blocks leaves no asphalt beside it.
        let unmarked = lanes < 2
            || road_width < 4
            || highway == "service"
            || is_pedestrian_grade(highway)
            || tags.get("service").map(String::as_str) == Some("parking_aisle");
        if unmarked {
            (1, false)
        } else if self == RoadDetail::Clean && lanes >= 5 {
            (3, true)
        } else {
            (2, false)
        }
    }

    /// Whether a zebra crossing cell at `coord` (along the road) is a white
    /// bar. `max` keeps the 1-on/1-off pattern as it always was; the others
    /// paint 2-on/2-off bars, which stay apart when seen from above, with a
    /// euclidean remainder so negative coordinates alternate too.
    pub fn zebra_bar(self, coord: i32) -> bool {
        if self == RoadDetail::Max {
            coord % 2 < 1
        } else {
            coord.rem_euclid(4) < 2
        }
    }
}

/// Perpendicular offset of divider `l` from the centre line. The double centre
/// line sits on the centre cell and the one beside it.
pub fn divider_offset(l: i32, lane_width: f32, half_width: f32, twin: bool) -> f32 {
    if twin {
        (l - 1) as f32
    } else {
        l as f32 * lane_width - half_width
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osm_parser::{ProcessedNode, ProcessedWay};

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn only_compact_skips_highways() {
        let footway = tags(&[("highway", "footway")]);
        assert!(!RoadDetail::Max.skips_highway(&footway));
        assert!(!RoadDetail::Clean.skips_highway(&footway));
        assert!(RoadDetail::Compact.skips_highway(&footway));
        for skipped in [
            &[("highway", "service")][..],
            &[("highway", "track")],
            &[("highway", "crossing")],
            &[
                ("highway", "traffic_signals"),
                ("crossing", "traffic_signals"),
            ],
            &[("highway", "residential"), ("footway", "crossing")],
        ] {
            assert!(
                RoadDetail::Compact.skips_highway(&tags(skipped)),
                "{skipped:?}"
            );
        }
        for kept in [
            &[("highway", "residential")][..],
            &[("highway", "primary"), ("footway", "no")],
            &[("highway", "street_lamp")],
            &[("building", "yes")],
        ] {
            assert!(!RoadDetail::Compact.skips_highway(&tags(kept)), "{kept:?}");
        }
    }

    #[test]
    fn compact_keeps_elements_another_handler_builds() {
        let node = |pairs: &[(&str, &str)]| {
            ProcessedElement::Node(ProcessedNode {
                id: 1,
                tags: tags(pairs),
                x: 0,
                z: 0,
            })
        };
        let way = |pairs: &[(&str, &str)]| {
            ProcessedElement::Way(ProcessedWay {
                id: 1,
                nodes: Vec::new(),
                tags: tags(pairs),
            })
        };
        assert!(RoadDetail::Compact.drops_element(&way(&[("highway", "footway")])));
        assert!(RoadDetail::Compact.drops_element(&node(&[("highway", "crossing")])));
        assert!(!RoadDetail::Clean.drops_element(&way(&[("highway", "footway")])));
        assert!(!RoadDetail::Compact
            .drops_element(&way(&[("highway", "pedestrian"), ("building", "yes")])));
        assert!(!RoadDetail::Compact
            .drops_element(&node(&[("highway", "bus_stop"), ("amenity", "shelter")])));
        assert!(!RoadDetail::Compact
            .drops_element(&node(&[("highway", "crossing"), ("barrier", "kerb")])));
    }

    #[test]
    fn max_matches_upstream_markings() {
        let none = HashMap::new();
        for lanes in 1..=8 {
            for width in 1..=12 {
                assert_eq!(
                    RoadDetail::Max.lane_plan("service", &none, lanes, width),
                    (lanes, false)
                );
            }
        }
        for scale in [0.3, 0.5, 1.0, 2.0] {
            assert_eq!(
                RoadDetail::Max.dash_length(scale),
                (5.0 * scale).ceil() as i32
            );
        }
        for x in -8..=8 {
            assert_eq!(RoadDetail::Max.zebra_bar(x), x % 2 < 1);
        }
        assert_eq!(divider_offset(1, 3.5, 3.5, false), 0.0);
    }

    #[test]
    fn clean_and_compact_simplify_lanes() {
        let none = HashMap::new();
        assert_eq!(
            RoadDetail::Clean.lane_plan("primary", &none, 4, 9),
            (2, false)
        );
        assert_eq!(
            RoadDetail::Clean.lane_plan("primary", &none, 6, 11),
            (3, true)
        );
        assert_eq!(
            RoadDetail::Compact.lane_plan("primary", &none, 6, 11),
            (2, false)
        );
        // Too narrow, unmarked, service and pedestrian ways get no stripe.
        assert_eq!(
            RoadDetail::Clean.lane_plan("primary", &none, 2, 3),
            (1, false)
        );
        assert_eq!(
            RoadDetail::Clean.lane_plan("primary", &none, 1, 9),
            (1, false)
        );
        assert_eq!(
            RoadDetail::Clean.lane_plan("service", &none, 2, 5),
            (1, false)
        );
        assert_eq!(
            RoadDetail::Clean.lane_plan("footway", &none, 2, 5),
            (1, false)
        );
        let aisle = tags(&[("service", "parking_aisle")]);
        assert_eq!(
            RoadDetail::Clean.lane_plan("unclassified", &aisle, 2, 5),
            (1, false)
        );
        // Twin dividers sit on the centre cell and its neighbour.
        assert_eq!(divider_offset(1, 3.7, 5.5, true), 0.0);
        assert_eq!(divider_offset(2, 3.7, 5.5, true), 1.0);
        assert_eq!(RoadDetail::Compact.dash_length(0.5), 4);
        assert_eq!(RoadDetail::Clean.dash_length(0.5), 3);
        assert_eq!(RoadDetail::Compact.dash_length(1.0), 5);
    }

    #[test]
    fn zebra_bars_alternate_across_zero() {
        let bars: Vec<bool> = (-8..8).map(|x| RoadDetail::Clean.zebra_bar(x)).collect();
        for pair in bars.chunks(2) {
            assert_eq!(pair[0], pair[1]);
        }
        for quad in bars.chunks(4) {
            assert_ne!(quad[0], quad[2]);
        }
        // The `max` remainder turns every negative cell into a bar.
        assert!((-8..0).all(|x| RoadDetail::Max.zebra_bar(x)));
    }
}
