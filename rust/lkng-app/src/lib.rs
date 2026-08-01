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
use lkng_location::{publishable_cell, Cell, LocationError};
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
    /// Coarse age band for filtering; 0 means unstated.
    pub age_band: u8,
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

/// Coarse, filterable attributes carried on a tile.
///
/// **Deliberately coarse.** Exact age, height and weight belong to the
/// profile, which its owner chooses to share. A tile is public to anyone
/// who subscribes to a cell, so putting precise demographics there would
/// hand a scraper a dossier on everyone in a neighbourhood. A band is
/// enough to filter a grid and far less useful to harvest.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TileFilters {
    /// 0 = unstated, else the decade band: 2 = 20s, 3 = 30s, …
    pub age_band: u8,
}

impl TileFilters {
    pub fn from_age(age: Option<u8>) -> Self {
        Self { age_band: age.map(|a| a / 10).unwrap_or(0) }
    }
}

/// What the user wants to see in the grid.
///
/// Filtering happens **client-side over already-public data**. It sends
/// nothing, so nobody learns what you are looking for — which matters more
/// here than in most apps, since search terms in this category are among
/// the most sensitive things a person types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GridFilter {
    /// Inclusive decade bands, e.g. `(2, 4)` for twenties through forties.
    pub age_bands: Option<(u8, u8)>,
    /// Case-insensitive substring over the headline.
    pub headline: Option<String>,
    /// Hide tiles from neighbouring cells.
    pub same_cell_only: bool,
}

impl GridFilter {
    pub fn is_empty(&self) -> bool {
        self.age_bands.is_none() && self.headline.is_none() && !self.same_cell_only
    }

    fn matches(&self, t: &Tile) -> bool {
        if self.same_cell_only && !t.same_cell {
            return false;
        }
        if let Some((lo, hi)) = self.age_bands {
            // Unstated (0) never matches a band filter: a filter that
            // silently includes people who said nothing is a lie.
            if t.age_band == 0 || t.age_band < lo || t.age_band > hi {
                return false;
            }
        }
        if let Some(h) = &self.headline {
            if !t.headline.to_lowercase().contains(&h.to_lowercase()) {
                return false;
            }
        }
        true
    }
}

/// A user session: identity, privacy setting, and the local block list.
///
/// Cloneable so UI frameworks can hold it in several places; the identity
/// is shared rather than duplicated, so key material still exists once.
#[derive(Clone)]
pub struct Session {
    identity: std::sync::Arc<Identity>,
    /// Device-local secret for jitter derivation. Never leaves the device;
    /// resetting it re-enables the averaging attack, so it is created once.
    jitter_secret: [u8; 32],
    pub privacy: Privacy,
    blocked: HashSet<[u8; 32]>,
}

impl Session {
    pub fn new(identity: Identity, jitter_secret: [u8; 32], privacy: Privacy) -> Self {
        Self {
            identity: std::sync::Arc::new(identity),
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

    pub fn unblock(&mut self, pseudonym: &[u8; 32]) {
        self.blocked.remove(pseudonym);
    }

    pub fn blocked_count(&self) -> usize {
        self.blocked.len()
    }

    /// Serialize the block list for local storage.
    ///
    /// Blocks are **device-local and never published**. Putting a block
    /// list on the network would tell the blocked person they were
    /// blocked, and hand everyone else a social graph of who avoids whom —
    /// which for this user base is genuinely dangerous. The cost is that
    /// blocks do not follow you to a new device unless this blob does; the
    /// identity backup is the right place for that, not a contract.
    pub fn export_blocks(&self) -> Vec<[u8; 32]> {
        let mut v: Vec<[u8; 32]> = self.blocked.iter().copied().collect();
        v.sort_unstable();
        v
    }

    pub fn import_blocks(&mut self, list: impl IntoIterator<Item = [u8; 32]>) {
        self.blocked.extend(list);
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
        self.compose_tile_with(params, headline, thumbnail, now_ms, TileFilters::default())
    }

    /// As [`Session::compose_tile`], with the coarse filter attributes
    /// others will filter your tile by.
    pub fn compose_tile_with(
        &self,
        params: &CellParams,
        headline: &str,
        thumbnail: Vec<u8>,
        now_ms: u64,
        filters: TileFilters,
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
            age_band: filters.age_band,
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
                age_band: rec.age_band,
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

    /// Tiles matching a filter, in render order.
    pub fn filtered(&self, filter: &GridFilter) -> Vec<Tile> {
        self.tiles().into_iter().filter(|t| filter.matches(t)).collect()
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

#[cfg(test)]
mod filter_tests {
    use super::*;

    const SF: (f64, f64) = (37.7749, -122.4194);
    const NOW: u64 = 1_785_527_000;

    fn sess(seed: u8) -> Session {
        Session::new(Identity::from_seed([seed; 32]), [seed ^ 0xFF; 32], Privacy::Km1)
    }

    fn grid_with(me: &Session, entries: &[(u8, &str, u8, bool)]) -> (Grid, Coverage) {
        let cov = me.coverage(SF.0, SF.1, NOW).unwrap();
        let home = cov.publish_target();
        let nb = CellParams {
            schema_v: SCHEMA_V,
            cell_id: cov.cells[1].as_str().to_string(),
            epoch: cov.epochs[0],
        };
        let mut g = Grid::new();
        for (seed, headline, band, same) in entries {
            let params = if *same { home.clone() } else { nb.clone() };
            let them = sess(*seed);
            let rec = them
                .compose_tile_with(
                    &params, headline, vec![*seed; 8], 100,
                    TileFilters { age_band: *band },
                )
                .unwrap();
            let mut st = CellState::default();
            st.insert(rec);
            let mut buf = Vec::new();
            ciborium::ser::into_writer(&st, &mut buf).unwrap();
            g.absorb(me, &params, &buf, &cov.home).unwrap();
        }
        (g, cov)
    }

    #[test]
    fn empty_filter_shows_everything() {
        let me = sess(1);
        let (g, _) = grid_with(&me, &[(10, "a", 2, true), (11, "b", 4, false)]);
        assert_eq!(g.filtered(&GridFilter::default()).len(), 2);
    }

    #[test]
    fn age_band_range_filters() {
        let me = sess(2);
        let (g, _) = grid_with(&me, &[(10, "twenties", 2, true), (11, "forties", 4, true)]);
        let only_20s = g.filtered(&GridFilter { age_bands: Some((2, 2)), ..Default::default() });
        assert_eq!(only_20s.len(), 1);
        assert_eq!(only_20s[0].headline, "twenties");
    }

    #[test]
    fn unstated_age_never_matches_a_band_filter() {
        // A filter that silently includes people who stated nothing is a lie.
        let me = sess(3);
        let (g, _) = grid_with(&me, &[(10, "quiet", 0, true)]);
        assert_eq!(g.filtered(&GridFilter { age_bands: Some((0, 9)), ..Default::default() }).len(), 0);
        assert_eq!(g.filtered(&GridFilter::default()).len(), 1, "but still visible unfiltered");
    }

    #[test]
    fn headline_search_is_case_insensitive() {
        let me = sess(4);
        let (g, _) = grid_with(&me, &[(10, "Bad Horror Films", 3, true), (11, "golf", 3, true)]);
        let hits = g.filtered(&GridFilter { headline: Some("horror".into()), ..Default::default() });
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn same_cell_only_hides_neighbours() {
        let me = sess(5);
        let (g, _) = grid_with(&me, &[(10, "here", 3, true), (11, "next door", 3, false)]);
        let near = g.filtered(&GridFilter { same_cell_only: true, ..Default::default() });
        assert_eq!(near.len(), 1);
        assert!(near[0].same_cell);
    }

    #[test]
    fn filters_compose_and_all_must_hold() {
        let me = sess(6);
        let (g, _) = grid_with(&me, &[
            (10, "horror fan", 2, true),
            (11, "horror fan", 4, true),
            (12, "horror fan", 2, false),
        ]);
        let q = GridFilter {
            age_bands: Some((2, 2)),
            headline: Some("horror".into()),
            same_cell_only: true,
        };
        assert_eq!(g.filtered(&q).len(), 1, "every criterion must hold");
    }

    #[test]
    fn age_band_is_coarse_not_exact() {
        // The privacy point: a tile carries a decade, never a birthday.
        for age in [30u8, 31, 39] {
            assert_eq!(TileFilters::from_age(Some(age)).age_band, 3);
        }
        assert_eq!(TileFilters::from_age(None).age_band, 0);
    }

    #[test]
    fn blocks_survive_export_and_import() {
        let mut a = sess(7);
        a.block([9u8; 32]);
        a.block([8u8; 32]);
        let saved = a.export_blocks();

        let mut b = sess(7);
        assert_eq!(b.blocked_count(), 0);
        b.import_blocks(saved);
        assert!(b.is_blocked(&[9u8; 32]) && b.is_blocked(&[8u8; 32]));

        b.unblock(&[9u8; 32]);
        assert!(!b.is_blocked(&[9u8; 32]));
        assert_eq!(b.blocked_count(), 1);
    }

    #[test]
    fn blocked_tiles_stay_hidden_under_every_filter() {
        let me_seed = 11u8;
        let mut me = sess(me_seed);
        let (g, _) = grid_with(&me, &[(10, "rude person", 3, true)]);
        let pseudonym = g.tiles()[0].pseudonym;
        me.block(pseudonym);
        // Rebuild with the block in place.
        let (g2, _) = grid_with(&me, &[(10, "rude person", 3, true)]);
        assert!(g2.filtered(&GridFilter::default()).is_empty());
        assert!(g2.filtered(&GridFilter { headline: Some("rude".into()), ..Default::default() }).is_empty());
    }
}
