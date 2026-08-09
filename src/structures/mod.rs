//! Generic Sponge .schem structure stamping (non-tree models such as the construction crane).

pub mod boat;
pub mod bushes;
pub mod car;
pub mod crane;
pub mod excavator;
pub mod fountain;
pub mod haybales;
#[allow(dead_code)] // parked-helicopter prop reserved: needs a seam-safe region-ownership guard
pub mod helicopter;
pub mod land_scatter;
pub mod lighthouse;
pub mod playground;
pub mod rocks;
pub mod schematic;
pub mod starship;
pub mod tombstone;
pub mod tractor;
pub mod windturbine;

/// One schematic-prop family. Order defines the PropSet bit index.
#[derive(Clone, Copy)]
#[repr(u16)]
pub enum Prop {
    Boat,
    Car,
    Crane,
    Excavator,
    Fountain,
    Helicopter,
    Lighthouse,
    Playground,
    Starship,
    Tombstone,
    Tractor,
    Windturbine,
}

impl Prop {
    fn from_name(s: &str) -> Option<Prop> {
        Some(match s {
            "boat" => Prop::Boat,
            "car" => Prop::Car,
            "crane" => Prop::Crane,
            "excavator" => Prop::Excavator,
            "fountain" => Prop::Fountain,
            "helicopter" => Prop::Helicopter,
            "lighthouse" => Prop::Lighthouse,
            "playground" => Prop::Playground,
            "starship" => Prop::Starship,
            "tombstone" => Prop::Tombstone,
            "tractor" => Prop::Tractor,
            "windturbine" => Prop::Windturbine,
            _ => return None,
        })
    }
}

/// Which schematic-prop families to place. Default = all (byte-identical to the
/// old always-on behaviour). Driven by the `--props all|none|comma-list` flag.
#[derive(Clone, Copy)]
pub struct PropSet(u16);

impl PropSet {
    pub const ALL: PropSet = PropSet(0x0FFF);
    pub const NONE: PropSet = PropSet(0);

    pub fn has(self, p: Prop) -> bool {
        self.0 & (1 << p as u16) != 0
    }

    /// Parse `all` | `none` | a comma list of family names. Unknown names are ignored.
    pub fn parse(s: &str) -> PropSet {
        let s = s.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("all") {
            return PropSet::ALL;
        }
        if s.eq_ignore_ascii_case("none") {
            return PropSet::NONE;
        }
        let mut bits = 0u16;
        for tok in s.split(',') {
            if let Some(p) = Prop::from_name(tok.trim().to_ascii_lowercase().as_str()) {
                bits |= 1 << p as u16;
            }
        }
        PropSet(bits)
    }

    /// The set to actually place for this run: [`PropSet::parse`], minus everything when
    /// the world is scaled down past `min_scale`.
    ///
    /// Props are bundled schematics at a fixed block size, so scaling the world does not
    /// scale them: at 1:10 a parked boat or crane keeps its real block footprint while the
    /// city around it shrank tenfold, and it reads as a monument. Skipping them below the
    /// threshold is the honest default; `--props-min-scale 0` places them at any scale.
    pub fn for_scale(spec: &str, scale: f64, min_scale: f64) -> PropSet {
        let set = PropSet::parse(spec);
        if min_scale > 0.0 && scale < min_scale && set.0 != 0 {
            println!(
                "Props skipped: schematic props do not scale down, and this world is 1:{:.0} \
                 (below --props-min-scale {min_scale}). Pass --props-min-scale 0 to place them anyway.",
                1.0 / scale.max(1e-6)
            );
            return PropSet::NONE;
        }
        set
    }
}

impl Default for PropSet {
    fn default() -> Self {
        PropSet::ALL
    }
}
