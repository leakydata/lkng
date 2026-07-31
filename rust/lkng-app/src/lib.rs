//! LKNG application logic — everything between the transport and the UI.
//!
//! Deliberately free of any UI framework and any specific backend, so the
//! rules that decide *what a user sees* are unit-testable without a
//! browser or a network. The Dioxus layer above this should contain
//! rendering and event wiring, and no policy.
//!
//! The policies that live here, each of which is a product decision:
//!
//! * which cells to watch (yours plus its eight neighbours — a tile one
//!   metre across a cell boundary is still nearby),
//! * which epochs to watch (current *and* previous, so a rollover never
//!   blanks the grid),
//! * what to drop before rendering (unverifiable tiles, your own tile,
//!   blocked pseudonyms, expired tiles),
//! * how to order what remains.

use std::collections::{BTreeMap, HashSet};

use lkng_identity::Identity;
use lkng_location::{publishable_cell, Cell, JitterRadius, LocationError};
use lkng_presence::{
    epoch_for_unix_time, verify::verify_self_contained, CellParams, CellState, PresenceRecord,
    MAX_HEADLINE_BYTES, MAX_THUMBNAIL_BYTES,
};

pub use lkng_location::JitterRadius as Privacy;

/// Schema version pinned into every cell's parameters.
pub const SCHEMA_V: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("location: {0}")]
    Location(#[from] LocationError),
    #[error("presence: {0}")]
    Presence(#[from] lkng_presence::PresenceError),
    #[error("identity: {0}")]
    Identity(#[from] lkng_identity::IdentityError),
    #[error("headline exceeds {MAX_HEADLINE_BYTES} bytes")]
    HeadlineTooLong,
    #[error("thumbnail exceeds {MAX_THUMBNAIL_BYTES} bytes")]
    ThumbnailTooLarge,
    #[error("decode: {0}")]
    Decode(String),
}

/// One rendered grid tile — what the UI actually draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    /// Per-epoch pseudonym. Stable within an epoch, so "the same person"
    /// is meaningful for the length of a session, and no longer.
    pub pseudonym: [u8; 32],
    pub headline: String,
    pub thumbnail: Vec<u8>,
    pub timestamp_ms: u64,
    /// Which cell this tile came from — `false` when it arrived from a
    /// neighbouring cell rather than your own.
    pub same_cell: bool,
}

/// Where the user is, expressed only as coarse cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub home: Cell,
    /// Home plus its eight neighbours: the cells to subscribe to.
    pub cells: Vec<Cell>,
    /// Current and previous epoch, so a rollover doesn't empty the grid.
    pub epochs: [u64; 2],
}

impl Coverage {
    /// Every `(cell, epoch)` pair to subscribe to — the full watch set.
    pub fn watch_set(&self) -> Vec<CellParams> {
        let mut out = Vec::with_capacity(self.cells.len() * 2);
        for c in &self.cells {
            for e in self.epochs {
                out.push(CellParams {
                    schema_v: SCHEMA_V,
                    cell_id: c.as_str().to_string(),
                    epoch: e,
                });
            }
        }
        out
    }

    /// Where a newly published tile goes: home cell, current epoch.
    pub fn publish_target(&self) -> CellParams {
        CellParams {
            schema_v: SCHEMA_V,
            cell_id: self.home.as_str().to_string(),
            epoch: self.epochs[0],
        }
    }
}

/// A user session: identity, privacy setting, and the local block list.
pub struct Session {
    identity: Identity,
    /// Device-local secret for jitter derivation. Never leaves the device;
    /// resetting it re-enables the averaging attack, so it is created once.
    jitter_secret: [u8; 32],
    pub privacy: Privacy,
    blocked: HashSet<[u8; 32]>,
}

impl Session {
    pub fn new(identity: Identity, jitter_secret: [u8; 32], privacy: Privacy) -> Self {
        Self {
            identity,
            jitter_secret,
            privacy,
            blocked: HashSet::new(),
        }
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// Block a pseudonym. Local-only and immediate: nothing is published,
    /// so the blocked party learns nothing.
    pub fn block(&mut self, pseudonym: [u8; 32]) {
        self.blocked.insert(pseudonym);
    }

    pub fn is_blocked(&self, pseudonym: &[u8; 32]) -> bool {
        self.blocked.contains(pseudonym)
    }

    /// Turn a real position into the coarse cells to watch.
    ///
    /// The only place raw coordinates are touched. Everything downstream
    /// sees cell strings, never latitude or longitude.
    pub fn coverage(&self, lat: f64, lon: f64, now_unix: u64) -> Result<Coverage, AppError> {
        let home = publishable_cell(&self.jitter_secret, lat, lon, self.privacy)?;
        let mut cells = vec![home.clone()];
        cells.extend(home.neighbours()?);
        let epoch = epoch_for_unix_time(now_unix);
        Ok(Coverage {
            home,
            cells,
            epochs: [epoch, epoch.saturating_sub(1)],
        })
    }

    /// Build a signed tile ready to publish into `params`.
    ///
    /// Signed by the epoch subkey, so publishing never exposes the durable
    /// identity. Caps are checked here rather than at the contract so the
    /// UI can report a problem before anything hits the network.
    pub fn compose_tile(
        &self,
        params: &CellParams,
        headline: &str,
        thumbnail: Vec<u8>,
        now_ms: u64,
    ) -> Result<PresenceRecord, AppError> {
        if headline.len() > MAX_HEADLINE_BYTES {
            return Err(AppError::HeadlineTooLong);
        }
        if thumbnail.len() > MAX_THUMBNAIL_BYTES {
            return Err(AppError::ThumbnailTooLarge);
        }
        let mut rec = PresenceRecord {
            pseudonym: [0; 32],
            headline: headline.to_string(),
            thumbnail,
            timestamp_ms: now_ms,
            verifying_key: None,
            writer_cert: None,
            sig: Vec::new(),
        };
        self.identity.sign_presence(&mut rec, params)?;
        // Never publish something we cannot ourselves verify.
        verify_self_contained(&rec, params)?;
        Ok(rec)
    }

    /// Serialize a tile as a contract delta.
    pub fn tile_delta(&self, rec: &PresenceRecord) -> Result<Vec<u8>, AppError> {
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&vec![rec.clone()], &mut buf)
            .map_err(|e| AppError::Decode(e.to_string()))?;
        Ok(buf)
    }

    /// My pseudonym in a given epoch — used to recognise and hide my own
    /// tile in the grid.
    pub fn my_pseudonym(&self, epoch: u64) -> [u8; 32] {
        self.identity.for_epoch(epoch).pseudonym()
    }
}

/// Accumulates verified tiles from many `(cell, epoch)` contracts into one
/// grid.
///
/// Deliberately a separate type from [`Session`]: the grid is rebuilt from
/// network state constantly, while a session is long-lived.
#[derive(Default)]
pub struct Grid {
    /// Keyed by pseudonym so a user who re-posts within an epoch appears
    /// once, with their newest tile.
    tiles: BTreeMap<[u8; 32], Tile>,
    /// Tiles that failed verification, counted rather than shown. A
    /// non-zero value here is a signal worth surfacing in diagnostics: it
    /// means someone is writing junk into a cell.
    pub rejected: usize,
}

impl Grid {
    pub fn new() -> Self {
        Self::default()
    }

    /// Absorb one cell's state.
    ///
    /// Every record is verified against the parameters it arrived under —
    /// the client does not trust the contract to have done it, because a
    /// peer could serve state that never passed through a validating node.
    pub fn absorb(
        &mut self,
        session: &Session,
        params: &CellParams,
        state_bytes: &[u8],
        home: &Cell,
    ) -> Result<(), AppError> {
        if state_bytes.is_empty() {
            return Ok(());
        }
        let state: CellState = ciborium::de::from_reader(state_bytes)
            .map_err(|e| AppError::Decode(e.to_string()))?;
        let mine = session.my_pseudonym(params.epoch);

        for rec in state.records.values() {
            if verify_self_contained(rec, params).is_err() {
                self.rejected += 1;
                continue;
            }
            if rec.pseudonym == mine || session.is_blocked(&rec.pseudonym) {
                continue;
            }
            let tile = Tile {
                pseudonym: rec.pseudonym,
                headline: rec.headline.clone(),
                thumbnail: rec.thumbnail.clone(),
                timestamp_ms: rec.timestamp_ms,
                same_cell: params.cell_id == home.as_str(),
            };
            // Keep the newest tile per person; ties break on pseudonym so
            // two clients rendering the same bytes agree.
            match self.tiles.get(&rec.pseudonym) {
                Some(existing) if existing.timestamp_ms >= tile.timestamp_ms => {}
                _ => {
                    self.tiles.insert(rec.pseudonym, tile);
                }
            }
        }
        Ok(())
    }

    /// Tiles ready to render: same-cell before neighbours, then newest
    /// first, then pseudonym as a deterministic tiebreak.
    ///
    /// Note what is *not* here: no distance, and no ordering derived from
    /// one. Position within the grid must never encode proximity beyond
    /// "your cell or next door".
    pub fn tiles(&self) -> Vec<Tile> {
        let mut v: Vec<Tile> = self.tiles.values().cloned().collect();
        v.sort_by(|a, b| {
            b.same_cell
                .cmp(&a.same_cell)
                .then(b.timestamp_ms.cmp(&a.timestamp_ms))
                .then(a.pseudonym.cmp(&b.pseudonym))
        });
        v
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SF: (f64, f64) = (37.7749, -122.4194);
    const NOW: u64 = 1_785_527_000;

    fn session(seed: u8) -> Session {
        Session::new(Identity::from_seed([seed; 32]), [seed ^ 0xFF; 32], Privacy::Km1)
    }

    fn state_with(recs: Vec<PresenceRecord>) -> Vec<u8> {
        let mut cell = CellState::default();
        for r in recs {
            cell.insert(r);
        }
        let mut buf = Vec::new();
        ciborium::ser::into_writer(&cell, &mut buf).unwrap();
        buf
    }

    #[test]
    fn coverage_watches_nine_cells_and_two_epochs() {
        let s = session(1);
        let c = s.coverage(SF.0, SF.1, NOW).unwrap();
        assert_eq!(c.cells.len(), 9, "home plus eight neighbours");
        assert_eq!(c.watch_set().len(), 18, "each cell in two epochs");
        assert_eq!(c.epochs[1], c.epochs[0] - 1, "previous epoch is watched too");
        assert_eq!(c.publish_target().cell_id, c.home.as_str());
    }

    #[test]
    fn coverage_never_exposes_coordinates() {
        let s = session(1);
        let c = s.coverage(SF.0, SF.1, NOW).unwrap();
        // A level-5 geohash is 5 chars; nothing finer may appear anywhere.
        for cell in &c.cells {
            assert_eq!(cell.as_str().len(), 5);
        }
        let rendered = format!("{:?}", c);
        assert!(!rendered.contains("37.77"), "latitude must not survive");
        assert!(!rendered.contains("122.4"), "longitude must not survive");
    }

    #[test]
    fn composed_tile_verifies_and_hides_durable_identity() {
        let s = session(2);
        let c = s.coverage(SF.0, SF.1, NOW).unwrap();
        let p = c.publish_target();
        let tile = s.compose_tile(&p, "hello", vec![1; 32], 1_000).unwrap();
        verify_self_contained(&tile, &p).unwrap();
        assert_ne!(
            tile.verifying_key.as_deref(),
            Some(s.identity().verifying_key_bytes().as_slice()),
            "a published tile must not carry the durable key"
        );
    }

    #[test]
    fn caps_rejected_before_the_network() {
        let s = session(2);
        let p = s.coverage(SF.0, SF.1, NOW).unwrap().publish_target();
        assert!(matches!(
            s.compose_tile(&p, &"x".repeat(MAX_HEADLINE_BYTES + 1), vec![], 1),
            Err(AppError::HeadlineTooLong)
        ));
        assert!(matches!(
            s.compose_tile(&p, "ok", vec![0; MAX_THUMBNAIL_BYTES + 1], 1),
            Err(AppError::ThumbnailTooLarge)
        ));
    }

    #[test]
    fn grid_hides_my_own_tile() {
        let me = session(3);
        let c = me.coverage(SF.0, SF.1, NOW).unwrap();
        let p = c.publish_target();
        let mine = me.compose_tile(&p, "me", vec![1; 8], 100).unwrap();

        let them = session(4);
        let theirs = them.compose_tile(&p, "them", vec![2; 8], 200).unwrap();

        let mut g = Grid::new();
        g.absorb(&me, &p, &state_with(vec![mine, theirs]), &c.home).unwrap();
        let tiles = g.tiles();
        assert_eq!(tiles.len(), 1, "my own tile must not appear in my grid");
        assert_eq!(tiles[0].headline, "them");
    }

    #[test]
    fn grid_drops_unverifiable_tiles_and_counts_them() {
        let me = session(5);
        let c = me.coverage(SF.0, SF.1, NOW).unwrap();
        let p = c.publish_target();
        let good = session(6).compose_tile(&p, "genuine", vec![1; 8], 100).unwrap();
        let mut forged = good.clone();
        forged.headline = "tampered".into(); // signature no longer matches

        let mut g = Grid::new();
        g.absorb(&me, &p, &state_with(vec![good, forged]), &c.home).unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g.tiles()[0].headline, "genuine");
        assert_eq!(g.rejected, 1, "junk is counted, not silently ignored");
    }

    #[test]
    fn grid_respects_blocks() {
        let mut me = session(7);
        let c = me.coverage(SF.0, SF.1, NOW).unwrap();
        let p = c.publish_target();
        let other = session(8);
        let theirs = other.compose_tile(&p, "blocked", vec![1; 8], 100).unwrap();
        me.block(theirs.pseudonym);

        let mut g = Grid::new();
        g.absorb(&me, &p, &state_with(vec![theirs]), &c.home).unwrap();
        assert!(g.is_empty(), "blocked pseudonyms never render");
    }

    #[test]
    fn one_tile_per_person_newest_wins() {
        let me = session(9);
        let c = me.coverage(SF.0, SF.1, NOW).unwrap();
        let p = c.publish_target();
        let them = session(10);
        let older = them.compose_tile(&p, "older", vec![1; 8], 100).unwrap();
        let newer = them.compose_tile(&p, "newer", vec![1; 8], 500).unwrap();

        let mut g = Grid::new();
        // Absorb in the "wrong" order — result must not depend on it.
        g.absorb(&me, &p, &state_with(vec![newer.clone()]), &c.home).unwrap();
        g.absorb(&me, &p, &state_with(vec![older]), &c.home).unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g.tiles()[0].headline, "newer");
    }

    #[test]
    fn same_cell_tiles_sort_before_neighbours() {
        let me = session(11);
        let c = me.coverage(SF.0, SF.1, NOW).unwrap();
        let home_p = c.publish_target();
        let neighbour = &c.cells[1];
        let nb_p = CellParams {
            schema_v: SCHEMA_V,
            cell_id: neighbour.as_str().to_string(),
            epoch: c.epochs[0],
        };

        // The neighbour's tile is NEWER, and must still sort second.
        let near = session(12).compose_tile(&home_p, "same cell", vec![1; 8], 100).unwrap();
        let far = session(13).compose_tile(&nb_p, "next door", vec![1; 8], 900).unwrap();

        let mut g = Grid::new();
        g.absorb(&me, &nb_p, &state_with(vec![far]), &c.home).unwrap();
        g.absorb(&me, &home_p, &state_with(vec![near]), &c.home).unwrap();
        let tiles = g.tiles();
        assert_eq!(tiles[0].headline, "same cell");
        assert_eq!(tiles[1].headline, "next door");
        assert!(tiles[0].same_cell && !tiles[1].same_cell);
    }

    #[test]
    fn tiles_carry_no_distance() {
        // Guard against a future "helpful" addition: the render model must
        // stay free of anything that encodes proximity.
        let me = session(14);
        let c = me.coverage(SF.0, SF.1, NOW).unwrap();
        let p = c.publish_target();
        let t = session(15).compose_tile(&p, "x", vec![1; 8], 1).unwrap();
        let mut g = Grid::new();
        g.absorb(&me, &p, &state_with(vec![t]), &c.home).unwrap();
        let rendered = format!("{:?}", g.tiles()[0]);
        for banned in ["distance", "meters", "metres", "feet", "miles", "km"] {
            assert!(!rendered.contains(banned), "tile leaked `{banned}`");
        }
    }
}
