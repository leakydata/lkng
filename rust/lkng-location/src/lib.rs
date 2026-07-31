//! On-device location privacy for LKNG.
//!
//! This module is the ONLY code allowed to see raw coordinates. Its single
//! public question is "what cell may I publish?" — the answer is a coarse
//! geohash string, and nothing finer ever crosses this boundary. In
//! production it runs inside the location delegate, so raw GPS and the
//! jitter secret are unreachable from the WebView even under XSS.
//!
//! ## The jitter rules (see docs/PLAN.md "Location privacy")
//!
//! * **Stable, never resampled.** Offsets are zero-mean; re-randomizing per
//!   update lets an observer average samples and recover the true point.
//!   The offset is derived with HKDF from `(local_secret, geohash_L4 of the
//!   true position)` — fixed per user per broad (~39 km) area, changing only
//!   when the user moves to a different broad area. NEVER derived from time.
//! * **Uniform publication rung.** Everyone publishes at [`CELL_PRECISION`]
//!   (geohash level 5, ~4.9 km): per-user cell sizes would put the most
//!   privacy-conscious users in the smallest crowds.
//! * **The user control is the jitter radius**, one of three rungs
//!   ([`JitterRadius`]), applied to the true point *before* geohashing.

use geohash::Coord;
use hkdf::Hkdf;
use sha2::Sha256;

/// Geohash precision of every published cell (level 5 ≈ 4.9 km × 4.9 km).
pub const CELL_PRECISION: usize = 5;
/// Geohash precision of the jitter-derivation area (level 4 ≈ 39 km × 19.5 km).
pub const JITTER_AREA_PRECISION: usize = 4;
/// HKDF domain-separation tag. Changing it re-derives every user's offset —
/// a wire-format break; do not touch.
const HKDF_INFO: &[u8] = b"lkng-location-jitter-v1";

const EARTH_RADIUS_M: f64 = 6_371_000.0;

/// User-selectable privacy rung: how far the published point may be moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JitterRadius {
    /// Publish the true cell.
    None,
    /// Offset up to ~1 km (default).
    #[default]
    Km1,
    /// Offset up to ~5 km — routinely lands in a neighbouring cell.
    Km5,
}

impl JitterRadius {
    pub fn meters(self) -> f64 {
        match self {
            JitterRadius::None => 0.0,
            JitterRadius::Km1 => 1_000.0,
            JitterRadius::Km5 => 5_000.0,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LocationError {
    #[error("coordinates out of range: lat {lat}, lon {lon}")]
    OutOfRange { lat: f64, lon: f64 },
    #[error("geohash: {0}")]
    Geohash(String),
}

/// A published cell — the only location-derived value that may leave the
/// device. Deliberately opaque: no accessor returns coordinates.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cell(String);

impl Cell {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// The 8 neighbouring cells (+ self = the 9-cell subscription set that
    /// fixes the geohash boundary problem).
    pub fn neighbours(&self) -> Result<Vec<Cell>, LocationError> {
        let n = geohash::neighbors(&self.0).map_err(|e| LocationError::Geohash(e.to_string()))?;
        Ok([n.n, n.ne, n.e, n.se, n.s, n.sw, n.w, n.nw]
            .into_iter()
            .map(Cell)
            .collect())
    }
}

fn check_range(lat: f64, lon: f64) -> Result<(), LocationError> {
    if !(lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)) {
        return Err(LocationError::OutOfRange { lat, lon });
    }
    Ok(())
}

fn geohash_at(lat: f64, lon: f64, precision: usize) -> Result<String, LocationError> {
    geohash::encode(Coord { x: lon, y: lat }, precision)
        .map_err(|e| LocationError::Geohash(e.to_string()))
}

/// Derive the stable offset (dx, dy in meters, uniform in the disc of
/// `radius_m`) for `local_secret` within the broad area containing the true
/// position. Same secret + same broad area → same offset, always.
fn stable_offset(local_secret: &[u8; 32], jitter_area: &str, radius_m: f64) -> (f64, f64) {
    if radius_m <= 0.0 {
        return (0.0, 0.0);
    }
    let hk = Hkdf::<Sha256>::new(Some(jitter_area.as_bytes()), local_secret);
    let mut okm = [0u8; 16];
    hk.expand(HKDF_INFO, &mut okm).expect("16 bytes is valid HKDF length");

    let u1 = u64::from_le_bytes(okm[..8].try_into().expect("8 bytes")) as f64 / u64::MAX as f64;
    let u2 = u64::from_le_bytes(okm[8..].try_into().expect("8 bytes")) as f64 / u64::MAX as f64;

    // Uniform in disc: r = R·sqrt(u1), θ = 2π·u2.
    let r = radius_m * u1.sqrt();
    let theta = 2.0 * std::f64::consts::PI * u2;
    (r * theta.cos(), r * theta.sin())
}

/// The one public question: given the true position, the user's jitter rung,
/// and the device-local secret, which cell may be published?
///
/// The secret must be 32 uniformly random bytes generated once on the device
/// (in production: inside the delegate, wrapped by the Android Keystore) and
/// NEVER regenerated casually — a reset re-enables the averaging attack.
pub fn publishable_cell(
    local_secret: &[u8; 32],
    true_lat: f64,
    true_lon: f64,
    radius: JitterRadius,
) -> Result<Cell, LocationError> {
    check_range(true_lat, true_lon)?;

    let jitter_area = geohash_at(true_lat, true_lon, JITTER_AREA_PRECISION)?;
    let (dx_m, dy_m) = stable_offset(local_secret, &jitter_area, radius.meters());

    // Meters → degrees at this latitude. cos(lat) can approach 0 at the
    // poles; clamp so longitude displacement stays finite.
    let dlat = (dy_m / EARTH_RADIUS_M).to_degrees();
    let coslat = true_lat.to_radians().cos().max(1e-6);
    let dlon = (dx_m / (EARTH_RADIUS_M * coslat)).to_degrees();

    let lat = (true_lat + dlat).clamp(-90.0, 90.0);
    let mut lon = true_lon + dlon;
    // Wrap longitude into [-180, 180].
    if lon > 180.0 {
        lon -= 360.0;
    } else if lon < -180.0 {
        lon += 360.0;
    }

    Ok(Cell(geohash_at(lat, lon, CELL_PRECISION)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const SECRET_A: [u8; 32] = [7u8; 32];
    const SECRET_B: [u8; 32] = [9u8; 32];

    #[test]
    fn no_jitter_is_true_cell() {
        let cell = publishable_cell(&SECRET_A, 37.7749, -122.4194, JitterRadius::None).unwrap();
        assert_eq!(cell.as_str(), geohash_at(37.7749, -122.4194, CELL_PRECISION).unwrap());
    }

    #[test]
    fn neighbours_are_nine_cell_set() {
        let cell = publishable_cell(&SECRET_A, 37.7749, -122.4194, JitterRadius::None).unwrap();
        let n = cell.neighbours().unwrap();
        assert_eq!(n.len(), 8);
        assert!(!n.contains(&cell));
    }

    #[test]
    fn out_of_range_rejected() {
        assert!(publishable_cell(&SECRET_A, 91.0, 0.0, JitterRadius::Km1).is_err());
        assert!(publishable_cell(&SECRET_A, f64::NAN, 0.0, JitterRadius::Km1).is_err());
    }

    proptest! {
        /// Determinism: same inputs, same cell — every time.
        #[test]
        fn deterministic(lat in -85.0f64..85.0, lon in -179.0f64..179.0) {
            let a = publishable_cell(&SECRET_A, lat, lon, JitterRadius::Km5).unwrap();
            let b = publishable_cell(&SECRET_A, lat, lon, JitterRadius::Km5).unwrap();
            prop_assert_eq!(a, b);
        }

        /// Stability: any two positions inside the SAME broad (L4) area get
        /// the SAME offset — repeated observation of a stationary-ish user
        /// reveals nothing new. (We assert via: offset function depends only
        /// on the L4 string.)
        #[test]
        fn offset_stable_within_area(lat in -85.0f64..85.0, lon in -179.0f64..179.0) {
            let area = geohash_at(lat, lon, JITTER_AREA_PRECISION).unwrap();
            let o1 = stable_offset(&SECRET_A, &area, 5_000.0);
            let o2 = stable_offset(&SECRET_A, &area, 5_000.0);
            prop_assert_eq!(o1, o2);
        }

        /// The offset never exceeds the chosen radius.
        #[test]
        fn offset_bounded(lat in -85.0f64..85.0, lon in -179.0f64..179.0) {
            let area = geohash_at(lat, lon, JITTER_AREA_PRECISION).unwrap();
            let (dx, dy) = stable_offset(&SECRET_A, &area, 1_000.0);
            prop_assert!((dx * dx + dy * dy).sqrt() <= 1_000.0 + 1e-9);
        }

        /// Different users in the same place get different offsets (with
        /// overwhelming probability) — the offset is per-secret, so one
        /// user's disclosure teaches nothing about another's.
        #[test]
        fn per_user_offsets_differ(lat in -85.0f64..85.0, lon in -179.0f64..179.0) {
            let area = geohash_at(lat, lon, JITTER_AREA_PRECISION).unwrap();
            let oa = stable_offset(&SECRET_A, &area, 5_000.0);
            let ob = stable_offset(&SECRET_B, &area, 5_000.0);
            prop_assert_ne!(oa, ob);
        }

        /// The published value is always a valid level-5 geohash and decodes
        /// to a point within a cell-diagonal + jitter radius of the truth —
        /// i.e., coarse, never precise.
        #[test]
        fn published_cell_is_coarse(lat in -80.0f64..80.0, lon in -179.0f64..179.0) {
            let cell = publishable_cell(&SECRET_A, lat, lon, JitterRadius::Km1).unwrap();
            prop_assert_eq!(cell.as_str().len(), CELL_PRECISION);
            let (decoded, _, _) = geohash::decode(cell.as_str()).unwrap();
            // Haversine-ish small-angle distance check
            let dlat_m = (decoded.y - lat).to_radians() * EARTH_RADIUS_M;
            let dlon_m = (decoded.x - lon).to_radians() * EARTH_RADIUS_M
                * lat.to_radians().cos().abs().max(1e-6);
            let dist = (dlat_m * dlat_m + dlon_m * dlon_m).sqrt();
            // L5 cell diagonal ≈ 7 km; + 1 km jitter + margin.
            prop_assert!(dist < 12_000.0, "published cell centre {dist} m from truth");
        }
    }
}
