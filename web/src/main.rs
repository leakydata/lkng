//! LKNG grid UI.
//!
//! Rendering and event wiring only — every decision about *what* appears
//! lives in `lkng-app`, where it is unit-tested. If a rule starts creeping
//! into this file, it belongs down there instead.
//!
//! The grid runs in one of two modes, chosen automatically:
//!
//! * **Live** when a Freenet node answers on the client API — tiles come
//!   from the cells around you and are pushed as they change.
//! * **Demo** otherwise, so the interface can be built and reviewed with
//!   no node, no account and no network. Demo tiles are still genuinely
//!   signed and genuinely verified; only delivery is faked.

mod chat;
mod net;
mod photo;

use dioxus::prelude::*;
use wasm_bindgen::JsCast;
use chat::Thread;
use lkng_app::{Coverage, Grid, Privacy, Session, Tile, SCHEMA_V};
use lkng_identity::Identity;
use lkng_inbox::InboxParams;
use lkng_presence::{CellParams, CellState};
use net::{Node, Status};

/// Styles are **inlined**, not linked.
///
/// Dioxus injects a `<link href="/assets/…">` at runtime, which is
/// root-absolute. That resolves under `dx serve` at `/`, but the app is
/// really served from `/v1/contract/web/<id>/` on a Freenet node, where a
/// root-absolute path points outside the contract and 404s — the app
/// renders as unstyled buttons. Inlining removes path resolution from the
/// picture entirely, and costs one small file's worth of HTML.
const CSS: &str = include_str!("../assets/lkng.css");

/// Visible build marker.
///
/// Exists because "is the phone running the new UI?" was answered wrong
/// twice by inference. A string on screen is not an inference.
pub const BUILD_MARKER: &str = "b56492";

/// Compiled presence-cell contract, embedded so the client can seed a cell
/// it does not host. Without the code travelling with the PUT there is no
/// way to write to a shared cell at all.
const CELL_WASM: &[u8] = include_bytes!(
    "../../contracts/presence-cell/target/wasm32-unknown-unknown/release/presence_cell.wasm"
);

/// Compiled inbox contract, embedded for the same reason as [`CELL_WASM`]:
/// the first message to someone whose inbox nobody near you hosts has to
/// carry the code, or the write has nowhere to land.
const INBOX_WASM: &[u8] = include_bytes!(
    "../../contracts/inbox/target/wasm32-unknown-unknown/release/inbox_contract.wasm"
);

/// Compiled moderation-feed contract.
const FEED_WASM: &[u8] = include_bytes!(
    "../../contracts/moderation/target/wasm32-unknown-unknown/release/moderation_contract.wasm"
);

/// Compiled album contract.
const ALBUM_WASM: &[u8] = include_bytes!(
    "../../contracts/album/target/wasm32-unknown-unknown/release/album_contract.wasm"
);

/// Storage key for this device's album key and generation.
const ALBUM_KEY_KEY: &str = "lkng.album.key.v1";

/// The album's symmetric key, created on first use.
///
/// Held in web storage rather than the Keystore vault, and that is a
/// deliberate downgrade: this key is *designed to be shared* — every person
/// granted the album has a copy. Sealing it beside the identity seed would
/// imply a secrecy it does not have, and would put a routinely-copied value
/// into the vault whose whole purpose is holding the one value that must
/// never be copied.
fn album_key() -> [u8; 32] {
    let store = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    if let Some(s) = &store {
        if let Ok(Some(hex)) = s.get_item(ALBUM_KEY_KEY) {
            if let Some(k) = decode_hex(&hex) {
                return k;
            }
        }
    }
    let mut k = [0u8; 32];
    getrandom_03::fill(&mut k).expect("platform CSPRNG");
    if let Some(s) = &store {
        let _ = s.set_item(ALBUM_KEY_KEY, &encode_hex(&k));
    }
    k
}

/// The feed a report goes to, and the one subscribed by default.
///
/// A single named feed rather than a global list, because "the feed" is a
/// choice the user should be able to change. Today there is one and it is
/// on by default; the contract shape means a second one costs nothing but a
/// name, and nobody has to be appointed to run either.
const BASELINE_FEED: &str = "baseline";

fn feed_params() -> lkng_moderation::FeedParams {
    lkng_moderation::FeedParams { schema_v: SCHEMA_V, feed: BASELINE_FEED.to_string() }
}

/// File a signed report about a pseudonym.
///
/// Signed with the **epoch** identity, never the durable one: a report
/// carries its verifying key in public, and signing with the durable key
/// would tie every report a person ever files to one permanent identity —
/// and through it to their profile address.
fn send_report(
    node: &Node,
    session: &Session,
    epoch: u64,
    subject: [u8; 32],
    reason: lkng_moderation::Reason,
    note: &str,
) -> Result<(), String> {
    let params = feed_params();
    let mut report = lkng_moderation::Report {
        subject,
        reason: reason.code(),
        note: note.chars().take(280).collect(),
        timestamp_ms: now_unix() * 1000,
        verifying_key: None,
        sig: Vec::new(),
    };
    session
        .identity()
        .for_epoch(epoch)
        .sign_report(&mut report, &params)
        .map_err(|e| e.to_string())?;

    let params_bytes = cbor(&params);
    let key = Node::key_for(FEED_WASM, &params_bytes);

    let mut seed_state = lkng_moderation::FeedState::default();
    seed_state.insert(report.clone());
    node.seed_once(FEED_WASM, &params_bytes, cbor(&seed_state));

    // A list of reports, which is what the contract decodes as a delta.
    // Encoding the FeedState map here would be rejected exactly as the
    // presence delta was.
    node.update(key, cbor(&vec![report]));
    Ok(())
}

fn main() {
    dioxus::launch(App);
}

/// Storage key for this device's identity seed.
const SEED_KEY: &str = "lkng.identity.seed.v1";
/// Storage key for the jitter secret. Separate from the identity so that
/// losing one does not imply losing the other.
const JITTER_KEY: &str = "lkng.jitter.secret.v1";

/// Load this device's identity, generating one on first run.
///
/// Prefers the Android **Keystore vault** exposed by the shell as
/// `LkngVault`; falls back to `localStorage` only where that bridge does
/// not exist, i.e. a desktop browser during development.
///
/// The distinction matters: the seed *is* the account — it derives the
/// signing key, every epoch subkey, the encryption key and the recovery
/// bundle — and there is no server anywhere that could revoke a stolen
/// one. On a phone it is sealed by a non-exportable Keystore key, so
/// copying app storage yields ciphertext that opens nowhere else.
///
/// Remaining gap, stated rather than hidden: script running inside our own
/// WebView can still ask the vault to unseal, because the seed has to
/// reach the WASM crypto. Closing that means moving signing behind the
/// bridge so the seed never crosses it. That is the right end state and a
/// much larger change.
fn load_or_create(key: &str) -> [u8; 32] {
    if let Some(seed) = vault_get(key) {
        return seed;
    }
    let mut seed = [0u8; 32];
    getrandom_03::fill(&mut seed).expect("platform CSPRNG");
    vault_put(key, &seed);
    seed
}

/// The `LkngVault` object injected by the Android shell, if present.
fn vault() -> Option<js_sys::Object> {
    let win = web_sys::window()?;
    let v = js_sys::Reflect::get(&win, &"LkngVault".into()).ok()?;
    v.dyn_into::<js_sys::Object>().ok()
}

fn vault_call(name: &str, args: &js_sys::Array) -> Option<wasm_bindgen::JsValue> {
    let v = vault()?;
    let f = js_sys::Reflect::get(&v, &name.into()).ok()?;
    let f = f.dyn_into::<js_sys::Function>().ok()?;
    f.apply(&v, args).ok()
}

fn vault_get(key: &str) -> Option<[u8; 32]> {
    let args = js_sys::Array::of1(&key.into());
    if let Some(res) = vault_call("get", &args) {
        if let Some(b64) = res.as_string() {
            if let Some(bytes) = decode_b64_32(&b64) {
                return Some(bytes);
            }
        }
    }

    // Nothing in the vault. Look in web storage — either because this is a
    // desktop dev build with no shell, or because this install predates
    // the vault.
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten())?;
    let hex = storage.get_item(key).ok().flatten()?;
    let seed = decode_hex(&hex)?;

    // MIGRATE. Without this an install that already had a seed keeps using
    // web storage forever: the fallback succeeds, so nothing ever writes to
    // the vault, and the security fix silently applies only to people who
    // installed after it. Existing users are exactly the ones whose keys
    // have been exposed longest.
    let args = js_sys::Array::of2(&key.into(), &encode_b64(&seed).into());
    if vault_call("put", &args).and_then(|v| v.as_bool()).unwrap_or(false) {
        // Only remove the plaintext copy once the sealed one is confirmed
        // stored — losing the seed is losing the account.
        let _ = storage.remove_item(key);
    }
    Some(seed)
}

fn vault_put(key: &str, seed: &[u8; 32]) {
    let args = js_sys::Array::of2(&key.into(), &encode_b64(seed).into());
    if vault_call("put", &args).and_then(|v| v.as_bool()).unwrap_or(false) {
        return;
    }
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(key, &encode_hex(seed));
    }
}

/// True when the identity is held by the device keystore rather than web
/// storage. Surfaced in the UI so the claim is visible, not assumed.
pub fn identity_is_sealed() -> bool {
    vault_call("isSealed", &js_sys::Array::new())
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn encode_b64(b: &[u8; 32]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = ((c[0] as u32) << 16)
            | ((*c.get(1).unwrap_or(&0) as u32) << 8)
            | (*c.get(2).unwrap_or(&0) as u32);
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if c.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn decode_b64_32(s: &str) -> Option<[u8; 32]> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = Vec::new();
    for ch in s.bytes() {
        if ch == b'=' { break; }
        let idx = T.iter().position(|&t| t == ch)? as u32;
        bits.push(idx);
    }
    let mut out = Vec::new();
    for c in bits.chunks(4) {
        let mut n = 0u32;
        for (i, v) in c.iter().enumerate() { n |= v << (18 - 6 * i); }
        out.push((n >> 16) as u8);
        if c.len() > 2 { out.push((n >> 8) as u8); }
        if c.len() > 3 { out.push(n as u8); }
    }
    out.try_into().ok()
}

fn encode_hex(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn decode_hex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// The signed-in user — a real, per-install identity.
fn me() -> Session {
    Session::new(
        Identity::from_seed(load_or_create(SEED_KEY)),
        load_or_create(JITTER_KEY),
        Privacy::Km1,
    )
}

/// Where the app believes the user is, and how sure it is.
impl Fix {
    /// Whether this is a position the user has actually asserted, and so one
    /// we may publish. The fallback area is not: publishing against it would
    /// place someone in a distant cell, visible to strangers, without them
    /// having gone anywhere or asked for anything.
    fn is_publishable(self) -> bool {
        matches!(self, Fix::Device | Fix::Manual)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Fix {
    /// From the device, via the coarse-location bridge.
    Device,
    /// Set by hand by the user.
    ///
    /// Treated as publishable, like a real fix. That is deliberate: a
    /// claimed position is unverifiable on this network *anyway* — the app
    /// runs on the user's hardware and nothing can stop them lying about
    /// where they are (see PLAN.md, "location fraud: accepted and
    /// documented"). Refusing to publish a manual location would not buy
    /// any integrity; it would only deny the honest uses — travelling,
    /// checking a neighbourhood before moving there, or simply not wanting
    /// to hand a dating app your GPS.
    ///
    /// It is a *separate* variant rather than a lie about `Device` so the
    /// UI can say which one is in force, and so no future code can confuse
    /// "the user told us" with "the platform told us".
    Manual,
    /// Permission granted but no position yet — Android has not returned
    /// a last-known fix.
    Waiting,
    /// No bridge (desktop dev) or permission refused.
    None,
}

/// Ask the Android shell for a coarse position.
fn device_position() -> Option<(f64, f64)> {
    let win = web_sys::window()?;
    let loc = js_sys::Reflect::get(&win, &"LkngLocation".into()).ok()?;
    let loc = loc.dyn_into::<js_sys::Object>().ok()?;
    let f = js_sys::Reflect::get(&loc, &"lastKnown".into()).ok()?;
    let f = f.dyn_into::<js_sys::Function>().ok()?;
    let s = f.call0(&loc).ok()?.as_string()?;
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

fn location_bridge_present() -> bool {
    web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w, &"LkngLocation".into()).ok())
        .map(|v| !v.is_undefined() && !v.is_null())
        .unwrap_or(false)
}

/// Seconds since the unix epoch, for choosing the presence epoch.
fn now_unix() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64
}

/// Coverage from the device where possible.
///
/// The fallback coordinates are a **placeholder, not a guess**: showing
/// someone a grid of people 3,000 km away as though they were nearby would
/// be worse than showing nothing, so the UI says which case it is in
/// rather than quietly pretending.
/// Parse a "lat, lon" pair, rejecting anything out of range.
///
/// Returns `None` rather than clamping: a clamped coordinate is a *different
/// place*, and silently relocating someone who mistyped is worse than telling
/// them the input was wrong.
fn parse_latlon(s: &str) -> Option<(f64, f64)> {
    let (a, b) = s.split_once(',')?;
    let lat: f64 = a.trim().parse().ok()?;
    let lon: f64 = b.trim().parse().ok()?;
    (lat.abs() <= 90.0 && lon.abs() <= 180.0).then_some((lat, lon))
}

/// Storage key for a hand-set location.
const MANUAL_LOC_KEY: &str = "lkng.location.manual.v1";

/// The user's hand-set position, if they have chosen one.
fn manual_position() -> Option<(f64, f64)> {
    let s = web_sys::window()?.local_storage().ok()??.get_item(MANUAL_LOC_KEY).ok()??;
    let (a, b) = s.split_once(',')?;
    let (lat, lon): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    // Reject out-of-range values rather than passing them to `coverage`,
    // which treats them as a programmer error and panics. This value comes
    // from storage, so it is input, not an invariant.
    (lat.abs() <= 90.0 && lon.abs() <= 180.0).then_some((lat, lon))
}

fn set_manual_position(v: Option<(f64, f64)>) {
    let Some(store) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return;
    };
    match v {
        Some((lat, lon)) => {
            let _ = store.set_item(MANUAL_LOC_KEY, &format!("{lat},{lon}"));
        }
        None => {
            let _ = store.remove_item(MANUAL_LOC_KEY);
        }
    }
}

fn coverage_for(s: &Session) -> (Coverage, Fix) {
    // A hand-set location wins over the device. If someone has explicitly
    // said where they want to appear, silently overriding them with GPS is
    // the app deciding it knows better about the one thing they most
    // clearly asked to control.
    let (pos, fix) = match manual_position() {
        Some(p) => (p, Fix::Manual),
        None => match device_position() {
        Some(p) => (p, Fix::Device),
        None if location_bridge_present() => ((37.7749, -122.4194), Fix::Waiting),
        None => ((37.7749, -122.4194), Fix::None),
        },
    };
    let cov = s
        .coverage(pos.0, pos.1, now_unix())
        .expect("coordinates from the platform are in range");
    (cov, fix)
}

/// Parameters of our inbox for one epoch.
///
/// # Why the inbox is addressed by the epoch key, not the durable one
///
/// A stranger who taps "message" on a tile knows exactly one thing about
/// you: the epoch verifying key that signed it. They cannot know your
/// durable key — that is the entire point of signing tiles with subkeys —
/// so a durable-addressed inbox is one they have no way to find. An earlier
/// version tried it anyway and failed on the network with "signature
/// verification failed", because the envelope was bound to one key while the
/// contract was addressed by another.
///
/// So the inbox rotates with the epoch, and the client watches the **current
/// and previous** epochs — the same trick the grid uses so a rollover never
/// empties it. A message sent moments before a rollover lands in the
/// previous-epoch inbox, which is still subscribed.
///
/// The cost, stated plainly: mail sent to an inbox older than two epochs
/// (12 h) is not collected. Epoch keys are *derived* from the master seed
/// rather than discarded, so nothing is cryptographically lost and a future
/// version can sweep further back; today it simply does not look.
fn inbox_params_for(session: &Session, epoch: u64) -> InboxParams {
    InboxParams::new(session.identity().for_epoch(epoch).verifying_key_bytes())
}

fn cbor(v: &impl serde::Serialize) -> Vec<u8> {
    let mut b = Vec::new();
    ciborium::ser::into_writer(v, &mut b).expect("cbor");
    b
}

/// Demo tiles — real signatures, fake delivery.
fn demo_tiles(session: &Session, cov: &Coverage) -> Vec<Tile> {
    let home = cov.publish_target();
    let neighbour = CellParams {
        schema_v: SCHEMA_V,
        cell_id: cov.cells[1].as_str().to_string(),
        epoch: cov.epochs[0],
    };
    let people: [(u8, &str, u64, bool); 12] = [
        (10, "new in town, show me somewhere good", 900, true),
        (11, "coffee?", 890, true),
        (12, "up late, bad films", 880, true),
        (13, "here for the weekend", 870, true),
        (16, "record shops and long walks", 860, true),
        (17, "just moved, know nobody", 850, true),
        (18, "gym then nothing", 840, true),
        (19, "ask me about my cat", 830, true),
        (20, "quiet one tonight", 820, true),
        (14, "next neighbourhood over", 990, false),
        (15, "walking the dog", 960, false),
        (21, "cycling home", 950, false),
    ];
    let mut grid = Grid::new();
    for (seed, headline, ts, same) in people {
        let params = if same { home.clone() } else { neighbour.clone() };
        let them = Session::new(Identity::from_seed([seed; 32]), [seed; 32], Privacy::Km1);
        let tile = them
            .compose_tile(&params, headline, vec![seed; 48], ts)
            .expect("compose");
        let mut state = CellState::default();
        state.insert(tile);
        grid.absorb(session, &params, &cbor(&state), &cov.home)
            .expect("absorb");
    }
    grid.tiles()
}

/// Build the grid from whatever the node has actually delivered.
fn live_tiles(session: &Session, cov: &Coverage, node: &Node) -> (Vec<Tile>, usize) {
    let mut grid = Grid::new();
    for params in cov.watch_set() {
        let key = Node::key_for(CELL_WASM, &cbor(&params));
        if let Some(bytes) = node.state_of(key.id()) {
            // `absorb` re-verifies every record: a peer can serve state
            // that never passed through a validating node.
            let _ = grid.absorb(session, &params, &bytes, &cov.home);
        }
    }
    (grid.tiles(), grid.rejected)
}

/// What the user has entered about themselves.
///
/// Held locally and published on save. Kept as a plain struct rather than
/// a `ProfileBody` so the editor can hold half-finished input without
/// anything half-finished ever being signable.
#[derive(Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Draft {
    display_name: String,
    headline: String,
    bio: String,
    age: String,
    position: u8,
    gender: String,
    hiv_status: String,
    thumbnail: Vec<u8>,
}

const DRAFT_KEY: &str = "lkng.profile.draft.v1";

fn load_draft() -> Draft {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(DRAFT_KEY).ok().flatten())
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default()
}

fn save_draft(d: &Draft) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        if let Ok(j) = serde_json::to_string(d) {
            let _ = s.set_item(DRAFT_KEY, &j);
        }
    }
}

/// Which screen is showing.
///
/// Navigation follows the pattern people already know from this category:
/// your own avatar sits in the top-left of the header, and tapping it
/// opens everything about *you* — profile, settings, account. The grid
/// stays the home screen, because that is what the app is for.
///
/// Two navigation surfaces, because they answer two different questions and
/// merging them made the app worse:
///
/// * the **tab bar** switches between the things you do — browsing, the
///   people who noticed you, your conversations;
/// * the **avatar menu** holds everything about *you* — profile, settings,
///   account.
///
/// This is the split every app in the category uses, and it is not
/// arbitrary: "edit my profile" is a rare, deliberate act, while "check my
/// messages" happens constantly. Putting both in one list makes the common
/// action slower and the rare one easier to hit by accident.
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Browse,
    Taps,
    Messages,
    Albums,
    Profile,
    Settings,
}

impl Tab {
    /// The tabs that appear in the bottom bar, in order.
    const BAR: [Tab; 4] = [Tab::Browse, Tab::Taps, Tab::Messages, Tab::Albums];

    fn label(self) -> &'static str {
        match self {
            Tab::Browse => "Browse",
            Tab::Taps => "Taps",
            Tab::Messages => "Messages",
            Tab::Albums => "Albums",
            Tab::Profile => "Profile",
            Tab::Settings => "Settings",
        }
    }

    /// Inline SVG rather than an icon font or image asset: the app is served
    /// from inside a contract, where every extra file is another path that
    /// has to resolve correctly on a node — the exact failure that once
    /// rendered the whole grid as unstyled buttons.
    fn icon(self) -> &'static str {
        match self {
            Tab::Browse => "▦",
            Tab::Taps => "✦",
            Tab::Messages => "✉",
            Tab::Albums => "❑",
            _ => "",
        }
    }
}


/// Storage key for the 18+ declaration.
const AGE_OK_KEY: &str = "lkng.age.confirmed.v1";

fn age_confirmed() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(AGE_OK_KEY).ok().flatten())
        .is_some()
}

fn set_age_confirmed() {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        // The flag, not the date. The date was needed to answer the
        // question; keeping it afterwards would store a more identifying
        // value than the answer requires, for no benefit.
        let _ = s.set_item(AGE_OK_KEY, "1");
    }
}

/// Today, as `(year, month, day)` in local time.
fn today_ymd() -> (i32, u32, u32) {
    let d = js_sys::Date::new_0();
    (d.get_full_year() as i32, d.get_month() + 1, d.get_date())
}

/// The 18+ gate, shown before anything else on first run.
///
/// Blocking rather than dismissible: a gate you can scroll past is not a
/// gate. It appears before the grid renders, so no tile is ever drawn for
/// someone who has not answered.
#[component]
fn AgeGate(onpass: EventHandler<()>) -> Element {
    let mut year = use_signal(String::new);
    let mut month = use_signal(String::new);
    let mut day = use_signal(String::new);
    let mut err = use_signal(|| None::<String>);

    rsx! {
        div { class: "gate",
            div { class: "gate-card",
                h1 { "LKNG" }
                p { class: "gate-lead", "You need to be 18 or over to use this app." }
                p { class: "hint",
                    "Your date of birth is checked on this device and stored nowhere. "
                    "It is never published, and no part of this app can verify it — "
                    "we ask because we would rather state the limit than pretend to "
                    "enforce it."
                }
                div { class: "dob",
                    input {
                        r#type: "number", placeholder: "DD", value: "{day}",
                        oninput: move |e| day.set(e.value()),
                    }
                    input {
                        r#type: "number", placeholder: "MM", value: "{month}",
                        oninput: move |e| month.set(e.value()),
                    }
                    input {
                        r#type: "number", placeholder: "YYYY", value: "{year}",
                        oninput: move |e| year.set(e.value()),
                    }
                }
                if let Some(m) = err() {
                    div { class: "warn", "{m}" }
                }
                button {
                    class: "primary wide",
                    onclick: move |_| {
                        let parsed = (
                            year.peek().trim().parse::<i32>().ok(),
                            month.peek().trim().parse::<u32>().ok(),
                            day.peek().trim().parse::<u32>().ok(),
                        );
                        let Some(born) = (match parsed {
                            (Some(y), Some(m), Some(d)) => Some((y, m, d)),
                            _ => None,
                        }) else {
                            err.set(Some("Please enter your full date of birth.".into()));
                            return;
                        };
                        match lkng_app::check_age(born, today_ymd()) {
                            lkng_app::AgeCheck::Ok => {
                                set_age_confirmed();
                                onpass.call(());
                            }
                            lkng_app::AgeCheck::TooYoung => err.set(Some(
                                "You need to be 18 or over to use LKNG.".into(),
                            )),
                            lkng_app::AgeCheck::Invalid => err.set(Some(
                                "That does not look like a date of birth.".into(),
                            )),
                        }
                    },
                    "Continue"
                }
                p { class: "hint",
                    "By continuing you agree this app publishes a photo and headline "
                    "to a public peer-to-peer network, which cannot be un-published. "
                    "Nothing is published until you add a headline."
                }
            }
        }
    }
}

#[component]
fn App() -> Element {
    let session = use_hook(me);
    let (cov, fix) = use_hook(|| coverage_for(&me()));
    let node = use_hook(Node::connect);

    // Once the socket opens, ask for every cell we watch and subscribe, so
    // later arrivals are pushed rather than polled.
    let mut requested = use_signal(|| false);
    let granted = use_signal(Vec::<([u8; 32], lkng_album::Grant)>::new);
    {
        let node = node.clone();
        let cov = cov.clone();
        let granted = granted.clone();
        use_effect(move || {
            let connected = *node.status.borrow() == Status::Connected;
            if connected && !requested() {
                for params in cov.watch_set() {
                    let key = Node::key_for(CELL_WASM, &cbor(&params));
                    node.get(key, true);
                }
                // Subscribe to our own inbox too, so a message that arrives
                // while the app is open lands without a poll. Requested with
                // the same call the grid uses; there is nothing special about
                // an inbox from the node's point of view.
                // Both epochs, for the same reason the grid watches both:
                // a message sent just before a rollover is addressed to the
                // older key, and there is no server to forward it.
                let session = me();
                for epoch in cov.epochs {
                    let key = Node::key_for(INBOX_WASM, &cbor(&inbox_params_for(&session, epoch)));
                    node.get(key, true);
                }
                requested.set(true);
            }

            // Fetch any album shared with us. Done here rather than on the
            // Albums tab so the photos are already in hand when the user
            // opens it -- a shared album that shows a spinner the first time
            // reads as broken.
            for (_, grant) in granted.read().iter() {
                let params = lkng_album::AlbumParams {
                    schema_v: SCHEMA_V,
                    address: grant.address,
                };
                let key = Node::key_for(ALBUM_WASM, &cbor(&params));
                node.get(key, true);
            }
        });
    }

    // Poll the shared inbox generation so the component re-renders when
    // anything lands. The callback-driven browser API has no stream to
    // await, so a cheap counter is the honest bridge.
    let mut generation = use_signal(|| 0u64);
    {
        let node = node.clone();
        use_future(move || {
            let node = node.clone();
            async move {
                loop {
                    let g = node.generation();
                    if g != *generation.peek() {
                        generation.set(g);
                    }
                    gloo_timers::future::TimeoutFuture::new(400).await;
                }
            }
        });
    }
    let _ = generation();

    // Publish our own presence, and keep it fresh.
    //
    // Re-published on a timer rather than once, for two reasons that both
    // make someone silently disappear from the grid if missed: cells are
    // capped at the newest N records, so a stale tile is eventually evicted
    // by newer arrivals; and cells are per-epoch contracts, so at a rollover
    // the tile has to be written into a *different* contract or the user
    // vanishes from their own neighbourhood.
    //
    // The interval is minutes, not seconds. Every publish is bytes that
    // every phone in the cell downloads, and the app's whole argument is
    // that it does not treat other people's batteries as free.
    {
        let node = node.clone();
        let cov = cov.clone();
        use_future(move || {
            let node = node.clone();
            let cov = cov.clone();
            async move {
                loop {
                    // Age gate first. The publish gate would already refuse a
                    // user with no headline, so this changes nothing today —
                    // but "nothing is published before you have answered" is
                    // a property that should not depend on a coincidence in
                    // a different function.
                    if *node.status.borrow() == Status::Connected && age_confirmed() {
                        let session = me();
                        let (cov_now, fix_now) = coverage_for(&session);
                        // Recomputed rather than captured: the epoch turns
                        // over while the app is open, and publishing into
                        // last epoch's cell is the same as not publishing.
                        let _ = publish_presence(
                            &node,
                            &session,
                            &cov_now,
                            &load_draft(),
                            fix_now,
                        );
                    }
                    let _ = &cov;
                    gloo_timers::future::TimeoutFuture::new(4 * 60 * 1000).await;
                }
            }
        });
    }

    let status = node.status.borrow().clone();
    let (found, rejected) = live_tiles(&session, &cov, &node);
    let live = !found.is_empty();
    let tiles = if live { found } else { demo_tiles(&session, &cov) };

    let mut age_ok = use_signal(age_confirmed);
    let mut tab = use_signal(|| Tab::Browse);
    let mut menu = use_signal(|| false);
    let draft = use_signal(load_draft);
    let mut blocked = use_signal(Vec::<[u8; 32]>::new);
    let mut selected = use_signal(|| None::<Tile>);
    let mut open_thread = use_signal(|| None::<[u8; 32]>);
    let mut compose = use_signal(String::new);
    let mut send_error = use_signal(|| None::<String>);
    let mut tapped = use_signal(|| false);
    let mut shared = use_signal(|| false);
    let mut reporting = use_signal(|| None::<[u8; 32]>);
    let mut album_msg = use_signal(|| None::<String>);
    let mut dismissed = use_signal(dismissed_backup);
    let _ = dismissed();

    // Our own album, as the node currently holds it.
    let my_album: lkng_album::AlbumState = {
        let params = album_params(&session);
        let key = Node::key_for(ALBUM_WASM, &cbor(&params));
        node.state_of(key.id())
            .and_then(|b| ciborium::de::from_reader(&b[..]).ok())
            .unwrap_or_default()
    };
    let mut report_done = use_signal(|| false);
    let visible: Vec<Tile> = tiles
        .into_iter()
        .filter(|t| !blocked.read().contains(&t.pseudonym))
        .collect();

    // Inbox: whatever the node currently holds for our own inbox contract,
    // decrypted here and nowhere else. Recomputed each render off the shared
    // generation counter, so a pushed message appears without a refresh.
    //
    // Blocked senders are filtered *before* anything is rendered: an inbox
    // is world-writable, so "blocked" has to mean invisible rather than
    // collapsed, or blocking someone still lets them occupy your screen.
    let threads: Vec<Thread> = {
        let mut all = Vec::new();
        for epoch in cov.epochs {
            let key = Node::key_for(INBOX_WASM, &cbor(&inbox_params_for(&session, epoch)));
            let state = node.state_of(key.id()).unwrap_or_default();
            all.extend(chat::threads_from_inbox(
                session.identity(),
                &state,
                &visible,
                &blocked.read(),
            ));
        }
        let network = chat::merge_threads(all);
        // Our own half of every conversation, which by design exists only
        // here — see the module docs on why there is no "sent" contract.
        chat::with_sent(network, &sent_log(), &visible)
    };

    // Albums other people have shared with us, from the same inbox states
    // the messages came from.
    let received_grants: Vec<([u8; 32], lkng_album::Grant)> = {
        let mut all = Vec::new();
        for epoch in cov.epochs {
            let key = Node::key_for(INBOX_WASM, &cbor(&inbox_params_for(&session, epoch)));
            if let Some(state) = node.state_of(key.id()) {
                all.extend(chat::grants_from_inbox(session.identity(), &state));
            }
        }
        // Newest grant per person: a re-share after a revocation supersedes
        // the old one, and offering both would hand the user a key that no
        // longer opens anything.
        let mut best: std::collections::BTreeMap<[u8; 32], lkng_album::Grant> = Default::default();
        for (from, g) in all {
            match best.get(&from) {
                Some(existing) if existing.generation >= g.generation => {}
                _ => {
                    best.insert(from, g);
                }
            }
        }
        best.into_iter().collect()
    };
    {
        // Feed the signal the fetch-effect reads. Compared first, so an
        // unchanged list cannot drive a re-render loop.
        let mut g = granted;
        if *g.peek() != received_grants {
            g.set(received_grants.clone());
        }
    }

    // A thread with nothing but taps belongs on the Taps screen, not in
    // Messages: it is a signal, not a conversation, and mixing them makes
    // the message list look full of things nobody actually said.
    let (taps, threads): (Vec<Thread>, Vec<Thread>) = threads
        .into_iter()
        .partition(|t| t.messages.iter().all(|m| m.kind == chat::Kind::Tap));

    // Mirrors the gate in `publish_presence` exactly. Kept as one expression
    // so the UI cannot claim visibility the publisher would refuse.
    let publish_state = lkng_app::publish_gate(
        status == Status::Connected,
        fix.is_publishable(),
        &draft.read().headline,
    );
    let visible_to_others = publish_state.is_ok();

    let note = match (&status, live) {
        (Status::Connected, true) => {
            "Live from the network. Everyone nearby, no distances, no company in the middle."
                .to_string()
        }
        (Status::Connected, false) => match fix {
            Fix::Device => "Connected — nobody in your area yet.".to_string(),
            Fix::Manual => {
                "Connected — nobody in the area you picked yet.".to_string()
            }
            Fix::Waiting => {
                "Connected. Waiting for your location — the grid below is a sample."
                    .to_string()
            }
            Fix::None => {
                "Connected, but location is unavailable, so this is a sample area."
                    .to_string()
            }
        },
        // Say where we are looking and why it failed. A status line that
        // only ever reads "connecting" cannot be told apart from a hang,
        // which cost real debugging time on device.
        (s, _) => format!(
            "{} Showing sample tiles — each is still really signed and verified.",
            s.describe(&net::node_url())
        ),
    };

    if !age_ok() {
        return rsx! {
            document::Meta {
                name: "viewport",
                content: "width=device-width, initial-scale=1, viewport-fit=cover",
            }
            style { dangerous_inner_html: CSS }
            AgeGate { onpass: move |_| age_ok.set(true) }
        };
    }

    rsx! {
        // viewport-fit=cover, or env(safe-area-inset-*) is always 0 and
        // the header sits underneath the phone's status bar. Set here
        // because the generated index.html ships a plain viewport meta.
        document::Meta {
            name: "viewport",
            content: "width=device-width, initial-scale=1, viewport-fit=cover",
        }
        style { dangerous_inner_html: CSS }
        header { class: "bar",
            button {
                class: "me",
                style: "{avatar_style(&draft.read().thumbnail)}",
                onclick: move |_| menu.set(!menu()),
                if draft.read().thumbnail.is_empty() { "+" }
            }
            div { class: "brand", "LKNG" }
            div { class: "cell",
                span { class: if live { "dot" } else { "dot off" } }
                "{cov.home.as_str()}"
                if !fix.is_publishable() {
                    span { class: "badge-sample", "sample" }
                }
            }
        }

        if menu() {
            div { class: "menu-backdrop", onclick: move |_| menu.set(false),
                nav { class: "menu", onclick: move |e| e.stop_propagation(),
                    div { class: "menu-head",
                        div { class: "me lg", style: "{avatar_style(&draft.read().thumbnail)}",
                            if draft.read().thumbnail.is_empty() { "+" }
                        }
                        div {
                            div { class: "menu-name",
                                if draft.read().display_name.is_empty() {
                                    "Set up your profile"
                                } else {
                                    "{draft.read().display_name}"
                                }
                            }
                            div { class: "menu-sub", "{cov.home.as_str()}" }
                        }
                    }
                    button { class: "menu-item",
                        onclick: move |_| { tab.set(Tab::Profile); menu.set(false); },
                        "Edit profile" }
                    button { class: "menu-item",
                        onclick: move |_| { tab.set(Tab::Settings); menu.set(false); },
                        "Settings and privacy" }
                    button { class: "menu-item",
                        onclick: move |_| { tab.set(Tab::Browse); menu.set(false); },
                        "Back to browsing" }
                }
            }
        }

        if tab() == Tab::Profile {
            ProfileEditor { draft: draft, onclose: move |_| tab.set(Tab::Browse) }
        }
        if tab() == Tab::Settings {
            SettingsPanel { onclose: move |_| tab.set(Tab::Browse) }
        }

        if tab() == Tab::Messages {
            main { class: "screen",
                h2 { class: "screen-h", "Messages" }
                if threads.is_empty() {
                    div { class: "empty",
                        "No messages yet."
                        br {}
                        span { class: "hint",
                            "Messages live on your device and in your inbox contract — "
                            "there is no server keeping a copy."
                        }
                    }
                }
                for th in threads.iter().cloned() {
                    button {
                        key: "{hex(&th.peer)}",
                        class: "thread",
                        onclick: move |_| open_thread.set(Some(th.peer)),
                        div { class: "thumb sm", style: "{peer_art(&th.peer, &visible)}" }
                        div { class: "thread-txt",
                            div { class: "thread-name", "{th.headline}" }
                            div { class: "thread-last",
                                {th.last().map(|m| m.body.clone()).unwrap_or_default()}
                            }
                        }
                    }
                }
            }
        }

        if tab() == Tab::Taps {
            main { class: "screen",
                h2 { class: "screen-h", "Taps" }
                if taps.is_empty() {
                    div { class: "empty",
                        "Nobody has tapped you yet."
                        br {}
                        span { class: "hint",
                            "A tap arrives as an encrypted envelope in your inbox, "
                            "identical on the wire to a message — so nobody "
                            "replicating it can tell who tapped whom, including us."
                        }
                    }
                }
                for th in taps.iter().cloned() {
                    button {
                        key: "tap-{hex(&th.peer)}",
                        class: "thread",
                        onclick: move |_| { open_thread.set(Some(th.peer)); tab.set(Tab::Messages); },
                        div { class: "thumb sm", style: "{peer_art(&th.peer, &visible)}" }
                        div { class: "thread-txt",
                            div { class: "thread-name", "{th.headline}" }
                            div { class: "thread-last", "tapped you" }
                        }
                    }
                }
            }
        }

        if tab() == Tab::Albums {
            main { class: "screen",
                h2 { class: "screen-h", "Album" }
                p { class: "hint",
                    "Photos here are encrypted on this device before they go "
                    "anywhere. Anyone can fetch the file; only people you share it "
                    "with can open it."
                }
                p { class: "hint",
                    "Sharing cannot be undone. Removing someone stops them seeing "
                    "anything you add afterwards — it cannot take back a photo they "
                    "have already opened, because that copy is on their phone now. "
                    "Share accordingly."
                }

                label { class: "photo-pick wide-pick",
                    input {
                        r#type: "file",
                        accept: "image/*",
                        onchange: {
                            let node = node.clone();
                            let album = my_album.clone();
                            move |evt: Event<FormData>| {
                                let files = evt.files();
                                let node = node.clone();
                                let album = album.clone();
                                spawn(async move {
                                    let Some(first) = files.into_iter().next() else { return };
                                    let Ok(bytes) = first.read_bytes().await else {
                                        album_msg.set(Some("could not read that file".into()));
                                        return;
                                    };
                                    // Re-encoded through the same canvas path the
                                    // profile photo uses, so EXIF -- including the
                                    // coordinates the photo was taken at -- cannot
                                    // survive into an album either.
                                    let arr = js_sys::Uint8Array::from(&bytes[..]);
                                    let parts = js_sys::Array::of1(&arr.buffer());
                                    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence(&parts)
                                    else {
                                        album_msg.set(Some("could not read that image".into()));
                                        return;
                                    };
                                    let clean = match photo::to_album_photo(&blob).await {
                                        Ok(b) => b,
                                        Err(e) => {
                                            album_msg.set(Some(e.to_string()));
                                            return;
                                        }
                                    };
                                    match add_album_photo(&node, &me(), &album, &clean) {
                                        Ok(()) => album_msg.set(Some(format!(
                                            "Added: {} bytes, encrypted before it left this device.",
                                            clean.len()
                                        ))),
                                        Err(e) => album_msg.set(Some(e)),
                                    }
                                });
                            }
                        },
                    }
                    span { "Add a photo to your album" }
                }

                if let Some(m) = album_msg() {
                    div { class: "sval", "{m}" }
                }

                div { class: "album-grid",
                    for (i, p) in my_album.readable_at(my_album.generation).iter().enumerate() {
                        div {
                            key: "ap-{i}",
                            class: "album-cell",
                            style: "{album_art(p)}",
                        }
                    }
                }

                if !received_grants.is_empty() {
                    h2 { class: "screen-h", "Shared with you" }
                    for (from, grant) in received_grants.iter().cloned() {
                        {
                            let params = lkng_album::AlbumParams {
                                schema_v: SCHEMA_V,
                                address: grant.address,
                            };
                            let key = Node::key_for(ALBUM_WASM, &cbor(&params));
                            let theirs: Option<lkng_album::AlbumState> = node
                                .state_of(key.id())
                                .and_then(|b| ciborium::de::from_reader(&b[..]).ok());
                            // Verified before anything is drawn. A grant names
                            // an address; without checking the album there is
                            // signed by the key in the grant, a grant would be
                            // an instruction to fetch and display a stranger's
                            // contract.
                            let ok = theirs
                                .as_ref()
                                .map(|a| lkng_album::verify::verify_album(a, &params).is_ok())
                                .unwrap_or(false);
                            let gk: [u8; 32] = grant.key[..].try_into().unwrap_or([0; 32]);
                            rsx! {
                                div { class: "shared-head",
                                    div { class: "thumb sm", style: "{peer_art(&from, &visible)}" }
                                    div { class: "thread-txt",
                                        div { class: "thread-name",
                                            {visible.iter().find(|t| t.pseudonym == from)
                                                .map(|t| t.headline.clone())
                                                .unwrap_or_else(|| "Someone nearby".into())}
                                        }
                                        div { class: "thread-last",
                                            if !ok {
                                                "not available"
                                            } else {
                                                "shared their album"
                                            }
                                        }
                                    }
                                }
                                if let (true, Some(album)) = (ok, theirs.as_ref()) {
                                    div { class: "album-grid",
                                        for (i, p) in album.readable_at(grant.generation)
                                            .iter().enumerate() {
                                            div {
                                                key: "sh-{i}",
                                                class: "album-cell",
                                                style: "{shared_art(&gk, p)}",
                                            }
                                        }
                                    }
                                    if album.readable_at(grant.generation).is_empty() {
                                        p { class: "hint",
                                            "Nothing here you can open. They may have "
                                            "changed who the album is shared with."
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                if my_album.photos.is_empty() {
                    div { class: "empty",
                        "Nothing in your album yet."
                    }
                } else {
                    p { class: "hint",
                        "{my_album.photos.len()} photo(s), generation "
                        "{my_album.generation}. Share it from someone's profile."
                    }
                }
            }
        }

        // The open conversation, over everything else.
        if let Some(peer) = open_thread() {
            {
                let th = threads.iter().find(|t| t.peer == peer).cloned();
                let tile = visible.iter().find(|t| t.pseudonym == peer).cloned();
                rsx! {
                    div { class: "convo",
                        header { class: "bar",
                            button { class: "linkish", onclick: move |_| open_thread.set(None),
                                "‹ Back" }
                            div { class: "brand",
                                {th.as_ref().map(|t| t.headline.clone())
                                    .unwrap_or_else(|| "Conversation".into())}
                            }
                        }
                        div { class: "msgs",
                            if let Some(t) = th.as_ref() {
                                for (i, m) in t.messages.iter().enumerate() {
                                    div {
                                        key: "{i}",
                                        class: match (m.outgoing, m.kind) {
                                            (_, chat::Kind::Tap) => "msg tap",
                                            (true, _) => "msg out",
                                            (false, _) => "msg in",
                                        },
                                        if m.kind == chat::Kind::Tap { "👋 tapped you" } else { "{m.body}" }
                                    }
                                }
                            }
                        }
                        if let Some(e) = send_error() {
                            div { class: "warn", "{e}" }
                        }
                        div { class: "composer",
                            input {
                                r#type: "text",
                                placeholder: "Message",
                                value: "{compose}",
                                oninput: move |e| compose.set(e.value()),
                            }
                            button {
                                class: "primary",
                                disabled: tile.as_ref().map(|t| t.encryption_key.is_none())
                                    .unwrap_or(true),
                                onclick: {
                                    let node = node.clone();
                                    let cov = cov.clone();
                                    move |_| {
                                        let Some(t) = tile.clone() else { return };
                                        let body = compose.peek().clone();
                                        if body.trim().is_empty() { return }
                                        match send_message(
                                            &node, &me(), &t, cov.epochs[0],
                                            chat::Kind::Text, &body,
                                        ) {
                                            Ok(()) => {
                                                compose.set(String::new());
                                                send_error.set(None);
                                            }
                                            Err(e) => send_error.set(Some(e)),
                                        }
                                    }
                                },
                                "Send"
                            }
                        }
                    }
                }
            }
        }

        main { class: if tab() == Tab::Browse { "" } else { "hidden" },
            p { class: "note", "{note}" }

            div { class: "grid",
                for tile in visible.iter().cloned() {
                    TileCard {
                        key: "{hex(&tile.pseudonym)}",
                        tile: tile.clone(),
                        onopen: move |t| selected.set(Some(t)),
                    }
                }
            }

            if visible.is_empty() {
                div { class: "empty",
                    "Nobody here right now. The grid fills as people arrive in your area."
                }
            }

            if rejected > 0 {
                div { class: "warn",
                    "{rejected} tile(s) failed verification and were not shown."
                }
            }
        }

        if let Some(t) = selected() {
            div { class: "sheet-backdrop", onclick: move |_| selected.set(None),
                div { class: "sheet", onclick: move |e| e.stop_propagation(),
                    div { class: "sheet-thumb", style: "{tile_art(&t)}" }
                    h2 { "{t.headline}" }
                    p { class: "meta",
                        if t.same_cell { "In your area" } else { "Next area over" }
                    }
                    p { class: "hint",
                        "You're seeing a temporary identity. Their profile stays private "
                        "until they choose to share it."
                    }
                    div { class: "actions",
                        button {
                            class: "primary",
                            // Two separate reasons to refuse, both real:
                            //
                            // 1. no encryption key on their tile — nothing to
                            //    seal to;
                            // 2. this is a *sample* tile. Sample identities
                            //    are derived from constant seeds compiled into
                            //    this binary, so their private keys are known
                            //    to anyone with the source. Sealing a message
                            //    to one would look like encryption and be
                            //    publicly readable — worse than refusing,
                            //    because the user would believe it was private.
                            disabled: t.encryption_key.is_none() || !live,
                            onclick: move |_| {
                                open_thread.set(Some(t.pseudonym));
                                tab.set(Tab::Messages);
                                selected.set(None);
                            },
                            "Message"
                        }
                        button {
                            class: "secondary",
                            disabled: t.encryption_key.is_none() || !live,
                            onclick: {
                                let node = node.clone();
                                let cov = cov.clone();
                                let t = t.clone();
                                move |_| {
                                    match send_message(
                                        &node, &me(), &t, cov.epochs[0],
                                        chat::Kind::Tap, "",
                                    ) {
                                        Ok(()) => { tapped.set(true); }
                                        Err(e) => send_error.set(Some(e)),
                                    }
                                }
                            },
                            if tapped() { "Tapped" } else { "Tap" }
                        }
                        button {
                            class: "secondary",
                            disabled: t.encryption_key.is_none() || !live
                                || my_album.photos.is_empty(),
                            onclick: {
                                let node = node.clone();
                                let cov = cov.clone();
                                let t = t.clone();
                                let gen = my_album.generation;
                                move |_| {
                                    match share_album(&node, &me(), &t, cov.epochs[0], gen) {
                                        Ok(()) => shared.set(true),
                                        Err(e) => send_error.set(Some(e)),
                                    }
                                }
                            },
                            if shared() { "Album shared" } else { "Share album" }
                        }
                        if !live {
                            p { class: "hint",
                                "This is a sample profile, not a real person, so it "
                                "can't be messaged."
                            }
                        } else if t.encryption_key.is_none() {
                            p { class: "hint",
                                "Their app is an older version that can't receive "
                                "encrypted messages yet."
                            }
                        }
                        button {
                            class: "danger",
                            onclick: move |_| {
                                blocked.write().push(t.pseudonym);
                                selected.set(None);
                            },
                            "Block"
                        }
                        button {
                            class: "linkish",
                            onclick: move |_| {
                                reporting.set(Some(t.pseudonym));
                                report_done.set(false);
                            },
                            "Report"
                        }
                    }
                }
            }
        }


        if let Some(subject) = reporting() {
            div { class: "sheet-backdrop", onclick: move |_| reporting.set(None),
                div { class: "sheet", onclick: move |e| e.stop_propagation(),
                    if report_done() {
                        h2 { "Reported" }
                        p { class: "hint",
                            "Your report is signed and published to the baseline feed. "
                            "It is pseudonymous, not anonymous: someone reading the feed "
                            "can see that the same temporary identity filed it, and that "
                            "identity is the one on your tile this epoch. Blocking them "
                            "is immediate and private, and is the thing that actually "
                            "stops you seeing them."
                        }
                        div { class: "actions",
                            button {
                                class: "danger",
                                onclick: move |_| {
                                    blocked.write().push(subject);
                                    reporting.set(None);
                                    selected.set(None);
                                },
                                "Block them too"
                            }
                            button { class: "linkish", onclick: move |_| reporting.set(None),
                                "Done" }
                        }
                    } else {
                        h2 { "Report" }
                        p { class: "hint",
                            "There is no company here to appeal to. A report is a signed "
                            "statement in a feed that other people choose to trust — it "
                            "does not remove anyone, and nothing but blocking will stop "
                            "them reaching you."
                        }
                        for reason in lkng_moderation::Reason::ORDER {
                            button {
                                key: "{reason.code()}",
                                class: "menu-item",
                                onclick: {
                                    let node = node.clone();
                                    let cov = cov.clone();
                                    move |_| {
                                        let _ = send_report(
                                            &node, &me(), cov.epochs[0], subject, reason, "",
                                        );
                                        report_done.set(true);
                                    }
                                },
                                "{reason.label()}"
                            }
                        }
                    }
                }
            }
        }

        nav { class: "tabs",
            for t in Tab::BAR {
                button {
                    key: "{t.label()}",
                    class: if tab() == t { "tab on" } else { "tab" },
                    onclick: move |_| { tab.set(t); open_thread.set(None); },
                    div { class: "tab-icon", "{t.icon()}" }
                    "{t.label()}"
                    // Count, not a red dot: a dot says "something happened"
                    // and makes people open the app to find out. A number is
                    // the same information without the manufactured urgency,
                    // which is the whole difference this app is arguing for.
                    if t == Tab::Messages && !threads.is_empty() {
                        span { class: "pip", "{threads.len()}" }
                    }
                    if t == Tab::Taps && !taps.is_empty() {
                        span { class: "pip", "{taps.len()}" }
                    }
                }
            }
        }

        // Whether *they* are in the grid, not just whether they can see it.
        // An app that publishes you to a public cell owes you a plain
        // statement of whether it has done so; "trust us" is the thing this
        // whole project exists to refuse.
        // Prompted once the account is worth something -- a headline written,
        // or a message received. Nagging someone on a blank first run, before
        // they have decided to stay, teaches them to dismiss the one banner
        // that actually matters.
        if tab() == Tab::Browse && !backup_saved() && !dismissed_backup()
            && (!draft.read().headline.trim().is_empty() || !threads.is_empty())
        {
            div { class: "banner",
                div {
                    b { "Back up your account" }
                    br {}
                    "There is no password reset here. If you lose this phone "
                    "without a backup file, this account is gone for good."
                }
                div { class: "row",
                    button {
                        class: "primary",
                        onclick: move |_| { tab.set(Tab::Settings); },
                        "Save a backup"
                    }
                    button {
                        class: "secondary",
                        onclick: move |_| { dismiss_backup(); dismissed.set(true); },
                        "Not now"
                    }
                }
            }
        }

        if tab() == Tab::Browse {
            div {
                class: if visible_to_others { "visnote on" } else { "visnote" },
                match publish_state {
                    Ok(()) => rsx! { "You're visible in {cov.home.as_str()}" },
                    Err(b) => rsx! { "{b.describe()}" },
                }
            }
        }

        footer { class: "foot",
            "{visible.len()} nearby · build {BUILD_MARKER}"
            br {}
            // Say which protection is actually in force. A user cannot
            // judge the risk of an app that will not tell them.
            if identity_is_sealed() {
                "your key is sealed by this device's keystore"
            } else {
                span { class: "warn-inline",
                    "pre-alpha: your key is in browser storage, not the device keystore"
                }
            }
        }
    }
}


/// Publish our own tile into the home cell.
///
/// # Why this is gated, and gated hard
///
/// Publishing presence is the one thing this app does that is **irrevocably
/// public**. A tile goes into a shared cell that anyone can subscribe to,
/// and once bytes are on the network they cannot be recalled. So it happens
/// only when every one of these is true:
///
/// * the user has actually filled in a headline — publishing an empty tile
///   puts someone in a public grid before they have decided to be there;
/// * we have a **real device fix**, not the sample area. Publishing against
///   the fallback location would drop the user into a cell on the other side
///   of the world, visible to strangers, having never left their house;
/// * the node is connected, so the write has somewhere to go.
///
/// The second condition is the one that would be easy to get wrong and hard
/// to notice, because in development the sample area *looks* like it works.
///
/// # What is deliberately not published
///
/// [`TileFilters::from_profile`] drops the health fields on the way in, so
/// HIV status cannot reach a tile even if a future editor adds it to the
/// draft. That is enforced by a test in `lkng-app`, not by this comment.
fn publish_presence(
    node: &Node,
    session: &Session,
    cov: &Coverage,
    draft: &Draft,
    fix: Fix,
) -> Result<(), String> {
    // The gate lives in `lkng-app` and is unit-tested there. Duplicating it
    // here as an `if` is how the UI and the publisher drift apart and start
    // disagreeing about whether someone is in the grid.
    lkng_app::publish_gate(true, fix.is_publishable(), &draft.headline)
        .map_err(|b| b.describe().to_string())?;

    let params = cov.publish_target();
    let filters = lkng_app::TileFilters {
        age_band: draft.age.parse::<u8>().ok().map(|a| a / 10).unwrap_or(0),
        position: draft.position,
    };
    let rec = session
        .compose_tile_with(
            &params,
            draft.headline.trim(),
            draft.thumbnail.clone(),
            now_unix() * 1000,
            filters,
        )
        .map_err(|e| e.to_string())?;

    let params_bytes = cbor(&params);
    let key = Node::key_for(CELL_WASM, &params_bytes);

    // Seed with a state containing our record, then send it again as a
    // delta. The seed handles the case where nobody near us hosts this cell
    // yet; the delta handles the case where the cell already exists and a
    // PUT would be ignored. Both are cheap, and the cell state is a
    // commutative merge, so doing both is safe rather than wasteful.
    let mut state = CellState::default();
    state.insert(rec.clone());
    // Seed only if this cell is new to us; the update is what carries the
    // tile either way. Re-seeding on every republish would push a whole
    // contract container into the network every few minutes for a contract
    // that already exists — paid for by every peer near us.
    node.seed_once(CELL_WASM, &params_bytes, cbor(&state));

    // The delta is a **list of records**, not a CellState. The contract
    // decodes `Vec<PresenceRecord>` and rejects a map outright:
    //
    //   delta: Semantic(None, "invalid type: map, expected array")
    //
    // Encoding it as state here silently published nothing — the write was
    // dispatched, the UI reported success, and the tile never landed.
    // `Session::tile_delta` is the encoder that gets this right; use it
    // rather than reconstructing the shape.
    let delta = session.tile_delta(&rec).map_err(|e| e.to_string())?;
    node.update(key, delta);
    Ok(())
}

/// Add a photo to this device's album, encrypting it before it goes near
/// the network.
///
/// The whole state is re-signed and republished, because an album is
/// single-writer: there is no merge, and a delta that only added would make
/// deleting a photo impossible.
fn add_album_photo(
    node: &Node,
    session: &Session,
    existing: &lkng_album::AlbumState,
    plaintext: &[u8],
) -> Result<(), String> {
    let mut nonce = [0u8; 24];
    getrandom_03::fill(&mut nonce).map_err(|e| e.to_string())?;

    let mut photo = Identity::seal_album_photo(&album_key(), plaintext, nonce)
        .map_err(|e| e.to_string())?;
    let generation = existing.generation.max(1);
    photo.generation = generation;
    photo.added_ms = now_unix() * 1000;

    let mut next = existing.clone();
    next.generation = generation;
    next.insert(photo);
    if next.photos.len() > lkng_album::MAX_PHOTOS {
        return Err(format!("an album holds at most {} photos", lkng_album::MAX_PHOTOS));
    }

    let params = album_params(session);
    session
        .identity()
        .sign_album(&mut next, &params)
        .map_err(|e| e.to_string())?;

    let params_bytes = cbor(&params);
    let key = Node::key_for(ALBUM_WASM, &params_bytes);
    node.seed_once(ALBUM_WASM, &params_bytes, cbor(&next));
    node.update(key, cbor(&next));
    Ok(())
}

fn album_params(session: &Session) -> lkng_album::AlbumParams {
    lkng_album::AlbumParams {
        schema_v: SCHEMA_V,
        address: lkng_album::address_of(&session.identity().verifying_key_bytes(), 0),
    }
}

/// Share the album with someone, by sealing the key into their inbox.
///
/// The grant is an ordinary envelope, so nobody replicating that inbox can
/// tell an album was shared, with whom, or that one exists.
fn share_album(
    node: &Node,
    session: &Session,
    tile: &Tile,
    epoch: u64,
    generation: u32,
) -> Result<(), String> {
    let grant = lkng_album::Grant {
        address: album_params(session).address,
        key: album_key().to_vec(),
        generation: generation.max(1),
        owner_vk: session.identity().verifying_key_bytes(),
    };
    let payload = grant.encode().map_err(|e| e.to_string())?;

    let enc = tile.encryption_key.ok_or("they cannot receive an album yet")?;
    let their_vk = tile.verifying_key.clone().ok_or("they cannot receive an album yet")?;
    let env = session
        .identity()
        .seal_message(&enc, &their_vk, epoch, &payload, now_unix() * 1000)
        .map_err(|e| e.to_string())?;

    let params = lkng_inbox::InboxParams::new(&their_vk);
    let params_bytes = cbor(&params);
    let key = Node::key_for(INBOX_WASM, &params_bytes);
    let mut delta = lkng_inbox::InboxState::default();
    delta.insert(env);
    node.seed_once(INBOX_WASM, &params_bytes, cbor(&lkng_inbox::InboxState::default()));
    node.update(key, cbor(&delta));
    Ok(())
}


/// Storage key recording that a backup file has been saved.
const BACKED_UP_KEY: &str = "lkng.backup.saved.v1";

/// Whether the user has waved the backup prompt away this install.
///
/// Remembered, not re-asked every launch. A prompt that returns after being
/// dismissed is one people learn to dismiss without reading, which is
/// exactly the wrong reflex to build for this particular warning.
fn dismissed_backup() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("lkng.backup.dismissed.v1").ok().flatten())
        .is_some()
}

fn dismiss_backup() {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item("lkng.backup.dismissed.v1", "1");
    }
}

fn backup_saved() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(BACKED_UP_KEY).ok().flatten())
        .is_some()
}

fn mark_backup_saved() {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        // Records only *that* a backup was made, never when or where it went.
        // A timestamp would let anyone with the device infer when the user
        // last had access to wherever they keep it.
        let _ = s.set_item(BACKED_UP_KEY, "1");
    }
}

/// Write a restored identity and its app data back into device storage.
///
/// Deliberately writes the seed through the same vault path a fresh install
/// uses, so a restored account is Keystore-sealed exactly like a new one. A
/// restore that quietly left the key in plain web storage would silently
/// downgrade the person who had most reason to trust the backup.
fn restore_into_device(id: &Identity, extra: &[u8]) {
    vault_put(SEED_KEY, &id.seed_bytes());

    let Ok(v) = serde_json::from_slice::<serde_json::Value>(extra) else {
        return;
    };
    if let Some(d) = v.get("draft") {
        if let Ok(draft) = serde_json::from_value::<Draft>(d.clone()) {
            save_draft(&draft);
        }
    }
    if let Some(k) = v.get("album_key").and_then(|k| k.as_str()) {
        if let Some(store) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = store.set_item(ALBUM_KEY_KEY, k);
        }
    }
    if let Some(sent) = v.get("sent").and_then(|s| s.as_array()) {
        for r in sent {
            let (Some(peer), Some(body), Some(ms)) = (
                r.get("peer").and_then(|p| p.as_str()).and_then(decode_hex),
                r.get("body").and_then(|b| b.as_str()),
                r.get("sent_ms").and_then(|m| m.as_u64()),
            ) else {
                continue;
            };
            chat::record_sent(chat::SentRecord {
                peer,
                tap: r.get("tap").and_then(|t| t.as_bool()).unwrap_or(false),
                body: body.to_string(),
                sent_ms: ms,
            });
        }
    }
}

/// Build an encrypted account backup.
///
/// # What is in it, and why the app data travels with the key
///
/// The 32-byte identity seed *is* the account — it derives the signing key,
/// every epoch subkey, the encryption key and the album key. Alongside it
/// goes the app data that exists nowhere else: the profile draft, sent
/// messages, favourites, notes and blocks. Those are not recoverable from
/// the network by design, so a backup that saved only the key would restore
/// an identity with no history, which is not what a person means by
/// "getting my account back".
///
/// # Why a passphrase, and why the strength warning is not decoration
///
/// The bundle is stretched with Argon2id (64 MiB, 3 passes) because it is a
/// file the user will put somewhere — a downloads folder, a cloud drive, a
/// message to themselves. Anyone who obtains it can attack it offline,
/// forever, at their own pace. Argon2id makes that expensive per guess; it
/// cannot make a common passphrase safe.
fn build_backup(session: &Session, passphrase: &str) -> Result<Vec<u8>, String> {
    let mut salt = [0u8; 16];
    getrandom_03::fill(&mut salt).map_err(|e| e.to_string())?;

    // Everything local that the network cannot give back.
    let extra = serde_json::to_vec(&serde_json::json!({
        "draft": load_draft(),
        "sent": chat::load_sent().iter().map(|r| serde_json::json!({
            "peer": hex(&r.peer),
            "tap": r.tap,
            "body": r.body,
            "sent_ms": r.sent_ms,
        })).collect::<Vec<_>>(),
        "album_key": encode_hex(&album_key()),
    }))
    .map_err(|e| e.to_string())?;

    session
        .identity()
        .to_backup_with(passphrase, salt, &extra)
        .map_err(|e| e.to_string())
}

/// Offer a byte blob to the user as a download.
///
/// A file, not a copyable string. The bundle is a few kilobytes of binary;
/// asking someone to select and paste that reliably is asking for a
/// truncated backup that only fails when they need it.
fn offer_download(bytes: &[u8], filename: &str) -> Result<(), String> {
    let arr = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&arr.buffer());
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts)
        .map_err(|_| "could not build the file".to_string())?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|_| "could not build the file".to_string())?;

    let doc = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let a = doc
        .create_element("a")
        .map_err(|_| "could not build the link".to_string())?;
    let _ = a.set_attribute("href", &url);
    let _ = a.set_attribute("download", filename);
    let el: web_sys::HtmlElement = a.dyn_into().map_err(|_| "bad element".to_string())?;
    el.click();
    let _ = web_sys::Url::revoke_object_url(&url);
    Ok(())
}

/// How weak a passphrase is, in words a person can act on.
///
/// Deliberately not a coloured bar with no explanation. The threat is
/// offline brute force against a file the user has stored somewhere, so the
/// advice that matters is length, and the message says so.
fn passphrase_warning(p: &str) -> Option<&'static str> {
    if p.chars().count() < 12 {
        Some("Too short. Use at least 12 characters — this file can be attacked \
              offline by anyone who gets a copy, for as long as they like.")
    } else if !p.contains(' ') && p.chars().count() < 16 {
        Some("Consider several unrelated words instead. Length beats symbols \
              against an offline attack.")
    } else {
        None
    }
}

/// Seal a message to a tile and write it into the recipient's inbox.
///
/// # Why this seeds before it updates
///
/// To UPDATE a contract your node does not host, the code, parameters and a
/// starting state have to travel with the request on the same session — a
/// bare update against an unknown contract is rejected with "missing
/// contract", which reads like a network fault and is not one. Messaging a
/// stranger is precisely the case where their inbox is unknown to us, so the
/// seed is the normal path here rather than a fallback.
///
/// The seeded state is an **empty** inbox. It cannot overwrite a real one:
/// inbox state is a commutative merge, so seeding an empty state next to an
/// existing one is a no-op, and the delta that follows carries the message.
fn send_message(
    node: &Node,
    session: &Session,
    tile: &Tile,
    epoch: u64,
    kind: chat::Kind,
    body: &str,
) -> Result<(), String> {
    let now = now_unix() * 1000;
    let (env, params) = chat::seal_to_tile(session.identity(), tile, epoch, kind, body, now)
        .map_err(|e| e.to_string())?;

    let params_bytes = cbor(&params);
    let key = Node::key_for(INBOX_WASM, &params_bytes);

    // Empty starting state, so the update has a contract to land in.
    let empty = lkng_inbox::InboxState::default();
    node.seed_once(INBOX_WASM, &params_bytes, cbor(&empty));

    // The delta is a state carrying exactly this envelope; the contract
    // merges it in. Deltas are self-contained for the same reason presence
    // records are (River #145) — a delta referring to something the peer has
    // never seen is dropped without a word.
    let mut delta = lkng_inbox::InboxState::default();
    delta.insert(env);
    node.update(key, cbor(&delta));

    // Record our own half locally. Done after the write is dispatched, so a
    // send that never leaves the node is not remembered as one that did.
    chat::record_sent(chat::SentRecord {
        peer: tile.pseudonym,
        tap: matches!(kind, chat::Kind::Tap),
        body: body.to_string(),
        sent_ms: now,
    });
    Ok(())
}

/// Reactive read of the local sent log.
///
/// Read through a signal so that sending re-renders the thread immediately
/// rather than after the next network generation tick — otherwise a message
/// appears to do nothing for up to half a second, which is exactly long
/// enough for someone to press send twice.
fn sent_log() -> Vec<chat::SentRecord> {
    chat::load_sent()
}

#[component]
fn TileCard(tile: Tile, onopen: EventHandler<Tile>) -> Element {
    let t = tile.clone();
    rsx! {
        button {
            class: "tile",
            onclick: move |_| onopen.call(t.clone()),
            div { class: "thumb", style: "{tile_art(&tile)}" }
            div { class: "overlay",
                div { class: "headline", "{tile.headline}" }
                if !tile.same_cell {
                    div { class: "badge", "next area" }
                }
            }
        }
    }
}

/// Background for a tile: their photo if they published one, otherwise a
/// deterministic swatch.
///
/// The fallback is not decoration. A grid of empty boxes reads as broken,
/// and a grid where the photo-less are invisible quietly punishes people who
/// have not uploaded one — including everybody on their first run.
///
/// The image is a `data:` URL built from bytes that were verified as part of
/// the record signature, so there is no request to any third party: rendering
/// the grid contacts nothing, and cannot be turned into a beacon by whoever
/// supplied the photo.
fn tile_art(tile: &Tile) -> String {
    if tile.thumbnail.is_empty() {
        swatch(&tile.pseudonym)
    } else {
        format!(
            "background-image:url(data:image/webp;base64,{});background-size:cover;\
             background-position:center",
            b64(&tile.thumbnail)
        )
    }
}

/// Background for one of our own album photos.
///
/// Decrypted here, in the browser, purely to draw it. The plaintext exists
/// only as a `data:` URL held for the life of the render — it is never
/// written back to storage or to any contract, because the encrypted copy is
/// the only one that should exist anywhere but the screen.
fn album_art(photo: &lkng_album::EncryptedPhoto) -> String {
    match Identity::open_album_photo(&album_key(), photo) {
        Ok(bytes) => format!(
            "background-image:url(data:image/webp;base64,{});background-size:cover;\
             background-position:center",
            b64(&bytes)
        ),
        // A photo we cannot open is one encrypted under a key this device no
        // longer has -- shown as a blank rather than hidden, so the count
        // stays honest.
        Err(_) => "background:#1b1d24".to_string(),
    }
}

/// Background for a photo in someone else's album, under their grant key.
///
/// A photo that will not open is drawn as a blank rather than skipped. It
/// means the owner rotated the key — the honest rendering of "there is
/// something here you are no longer meant to see" is an empty frame, not a
/// silently shorter grid that hides the fact anything changed.
fn shared_art(key: &[u8; 32], photo: &lkng_album::EncryptedPhoto) -> String {
    match Identity::open_album_photo(key, photo) {
        Ok(bytes) => format!(
            "background-image:url(data:image/webp;base64,{});background-size:cover;\
             background-position:center",
            b64(&bytes)
        ),
        Err(_) => "background:#1b1d24".to_string(),
    }
}

/// Art for a pseudonym we may or may not still have a tile for.
///
/// Falls back to the swatch when the person has left the grid — which
/// happens routinely, since tiles expire and pseudonyms rotate every six
/// hours. A conversation must keep rendering after that: losing the picture
/// is expected, losing the thread would be a bug.
fn peer_art(peer: &[u8; 32], tiles: &[Tile]) -> String {
    match tiles.iter().find(|t| &t.pseudonym == peer) {
        Some(t) => tile_art(t),
        None => swatch(peer),
    }
}

/// Placeholder tile art derived from the pseudonym. Deterministic per
/// pseudonym, and it changes when the pseudonym rotates — which is the
/// honest behaviour.
fn swatch(pseudonym: &[u8; 32]) -> String {
    // u32, not u16: 255 * 360 = 91_800 overflows a u16, which panics in
    // debug and silently wraps in release — the worst kind of difference.
    let h = u32::from(pseudonym[0]) * 360 / 255;
    let h2 = (h + 40) % 360;
    format!("linear-gradient(135deg, hsl({h} 55% 42%), hsl({h2} 45% 26%))")
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// Profile editor.
///
/// Everything here is optional except the age gate. A required field in an
/// app like this is a field people lie in, and a lie in a profile is worse
/// than a blank.
#[component]
fn ProfileEditor(draft: Signal<Draft>, onclose: EventHandler<()>) -> Element {
    let mut status = use_signal(String::new);
    let mut busy = use_signal(|| false);

    let pick_photo = move |evt: Event<FormData>| {
        let files = evt.files();
        spawn(async move {
            let Some(first) = files.into_iter().next() else { return };
            let Ok(bytes) = first.read_bytes().await else {
                status.set("could not read that file".into());
                return;
            };
            busy.set(true);
            status.set("processing…".into());

            let arr = js_sys::Uint8Array::from(&bytes[..]);
            let parts = js_sys::Array::of1(&arr.buffer());
            match web_sys::Blob::new_with_u8_array_sequence(&parts) {
                Ok(blob) => match photo::to_thumbnail(&blob).await {
                    Ok(thumb) => {
                        let n = thumb.len();
                        draft.write().thumbnail = thumb;
                        save_draft(&draft.read());
                        status.set(format!("photo ready ({n} bytes, location data removed)"));
                    }
                    Err(e) => status.set(e.to_string()),
                },
                Err(_) => status.set("could not read that image".into()),
            }
            busy.set(false);
        });
    };

    let save = move |_| {
        let d = draft.read().clone();
        // The one hard rule. 18+ is a legal obligation in every
        // jurisdiction this could ship in, and unlike the rest it cannot
        // be left blank.
        match d.age.parse::<u8>() {
            Ok(a) if (18..=120).contains(&a) => {}
            _ => {
                status.set("please enter your age (18 or over)".into());
                return;
            }
        }
        if d.display_name.trim().is_empty() {
            status.set("please choose a name to show".into());
            return;
        }
        save_draft(&d);
        status.set("saved to this device — publishing lands with the next build".into());
    };

    let thumb_style = {
        let d = draft.read();
        if d.thumbnail.is_empty() {
            "background: linear-gradient(135deg,#2a2d36,#181a20)".to_string()
        } else {
            format!("background-image:url(data:image/webp;base64,{})", b64(&d.thumbnail))
        }
    };

    rsx! {
        section { class: "editor",
            header { class: "bar",
                div { class: "brand", "Your profile" }
                button { class: "linkish", onclick: move |_| onclose.call(()), "Done" }
            }

            div { class: "form",
                label { class: "photo-pick", style: "{thumb_style}",
                    input {
                        r#type: "file",
                        accept: "image/*",
                        onchange: pick_photo,
                    }
                    if draft.read().thumbnail.is_empty() {
                        span { "Add a photo" }
                    }
                }
                p { class: "hint",
                    "Your photo is resized on this device and its location data is removed "
                    "before it is ever published. It will be visible to anyone nearby."
                }

                Field { label: "Name shown", value: draft.read().display_name.clone(),
                    oninput: move |v| { draft.write().display_name = v; } }
                Field { label: "Headline", value: draft.read().headline.clone(),
                    oninput: move |v| { draft.write().headline = v; } }
                Field { label: "Age", value: draft.read().age.clone(),
                    oninput: move |v| { draft.write().age = v; } }
                Field { label: "Gender", value: draft.read().gender.clone(),
                    oninput: move |v| { draft.write().gender = v; } }

                div { class: "field",
                    span { class: "flabel", "Position" }
                    div { class: "chips",
                        for (code, name) in [
                            (1u8, "Top"), (2, "Vers Top"), (3, "Versatile"),
                            (4, "Vers Bottom"), (5, "Bottom"), (6, "Side"),
                        ] {
                            button {
                                class: if draft.read().position == code { "chip on" } else { "chip" },
                                onclick: move |_| {
                                    let cur = draft.read().position;
                                    draft.write().position = if cur == code { 0 } else { code };
                                },
                                "{name}"
                            }
                        }
                    }
                }

                div { class: "field",
                    span { class: "flabel", "Status" }
                    div { class: "chips",
                        for name in ["Negative", "Negative, on PrEP", "Positive", "Positive, undetectable"] {
                            button {
                                class: if draft.read().hiv_status == name { "chip on" } else { "chip" },
                                onclick: move |_| {
                                    let cur = draft.read().hiv_status.clone();
                                    draft.write().hiv_status =
                                        if cur == name { String::new() } else { name.to_string() };
                                },
                                "{name}"
                            }
                        }
                    }
                    p { class: "hint",
                        "Health information stays in your profile and is never put on the "
                        "public grid. Only people you share your profile with can see it."
                    }
                }

                div { class: "field",
                    span { class: "flabel", "About you" }
                    textarea {
                        rows: 4,
                        value: "{draft.read().bio}",
                        oninput: move |e| { draft.write().bio = e.value(); },
                    }
                }

                if !status().is_empty() {
                    p { class: "status", "{status}" }
                }

                button {
                    class: "primary wide",
                    disabled: busy(),
                    onclick: save,
                    if busy() { "Working…" } else { "Save" }
                }
            }
        }
    }
}

#[component]
fn Field(label: String, value: String, oninput: EventHandler<String>) -> Element {
    rsx! {
        div { class: "field",
            span { class: "flabel", "{label}" }
            input {
                r#type: "text",
                value: "{value}",
                oninput: move |e| oninput.call(e.value()),
            }
        }
    }
}

fn b64(b: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in b.chunks(3) {
        let n = ((c[0] as u32) << 16)
            | ((*c.get(1).unwrap_or(&0) as u32) << 8)
            | (*c.get(2).unwrap_or(&0) as u32);
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 { T[(n >> 6 & 63) as usize] as char } else { '=' });
        out.push(if c.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// Avatar background: the user's own photo, or a plus sign to add one.
fn avatar_style(thumb: &[u8]) -> String {
    if thumb.is_empty() {
        "background: linear-gradient(135deg,#2a2d36,#181a20)".into()
    } else {
        format!("background-image:url(data:image/webp;base64,{})", b64(thumb))
    }
}

/// Settings and privacy.
///
/// Deliberately leads with what the app is doing on the user's behalf —
/// the node, the location precision, where their key lives — rather than
/// with preferences. Someone deciding whether to trust this needs those
/// facts first, and they are exactly what a conventional settings screen
/// buries.
#[component]
fn SettingsPanel(onclose: EventHandler<()>) -> Element {
    let mut manual = use_signal(|| {
        manual_position().map(|(a, b)| format!("{a}, {b}")).unwrap_or_default()
    });
    let mut loc_msg = use_signal(|| None::<String>);
    let mut pass = use_signal(String::new);
    let mut backup_msg = use_signal(|| None::<String>);
    rsx! {
        section { class: "editor",
            header { class: "bar",
                div { class: "brand", "Settings and privacy" }
                button { class: "linkish", onclick: move |_| onclose.call(()), "Done" }
            }
            div { class: "form",
                div { class: "setting",
                    div { class: "sname", "Your key" }
                    div { class: "sval",
                        if identity_is_sealed() {
                            "Sealed by this device's keystore"
                        } else {
                            "In browser storage — pre-alpha"
                        }
                    }
                }
                div { class: "setting",
                    div { class: "sname", "Your location" }
                    div { class: "sval",
                        "Coarse only. Your position becomes a roughly 5 km area on this "
                        "device before anything is shared, with a random offset. No "
                        "distance is ever calculated or published."
                    }
                    div { class: "sval",
                        "You can set it by hand instead — useful when travelling, or if "
                        "you would rather not give a dating app your GPS at all. Nothing "
                        "on this network can verify anyone's location, including yours, "
                        "so this takes nothing away from anybody."
                    }
                    input {
                        r#type: "text",
                        placeholder: "latitude, longitude — e.g. 51.5074, -0.1278",
                        value: "{manual}",
                        oninput: move |e| manual.set(e.value()),
                    }
                    div { class: "row",
                        button {
                            class: "primary",
                            onclick: move |_| {
                                let v = manual.peek().clone();
                                match parse_latlon(&v) {
                                    Some(p) => {
                                        set_manual_position(Some(p));
                                        loc_msg.set(Some(
                                            "Saved. Reopen the app to move to that area."
                                                .into(),
                                        ));
                                    }
                                    None => loc_msg.set(Some(
                                        "That does not look like a latitude and longitude."
                                            .into(),
                                    )),
                                }
                            },
                            "Use this location"
                        }
                        button {
                            class: "secondary",
                            onclick: move |_| {
                                set_manual_position(None);
                                manual.set(String::new());
                                loc_msg.set(Some("Back to using this device's location.".into()));
                            },
                            "Use my device"
                        }
                    }
                    if let Some(m) = loc_msg() {
                        div { class: "sval", "{m}" }
                    }
                }
                div { class: "setting",
                    div { class: "sname", "Back up your account" }
                    div { class: "sval",
                        "Your account is a key on this device. There is no server "
                        "holding a copy, no password reset and nobody who can let "
                        "you back in — if you lose this phone without a backup, "
                        "the account is gone permanently."
                    }
                    div { class: "sval",
                        "This saves an encrypted file containing your key, your "
                        "profile and your message history. Keep it somewhere you "
                        "will still have if the phone is lost."
                    }
                    input {
                        r#type: "password",
                        placeholder: "passphrase for the backup file",
                        value: "{pass}",
                        oninput: move |e| pass.set(e.value()),
                    }
                    if let Some(w) = passphrase_warning(&pass()) {
                        div { class: "warn", "{w}" }
                    }
                    div { class: "row",
                        button {
                            class: "primary",
                            disabled: pass().chars().count() < 12,
                            onclick: move |_| {
                                match build_backup(&me(), &pass.peek().clone())
                                    .and_then(|b| {
                                        let n = b.len();
                                        offer_download(&b, "lkng-account-backup.bin").map(|_| n)
                                    })
                                {
                                    Ok(n) => { mark_backup_saved(); backup_msg.set(Some(format!(
                                        "Saved {n} bytes. Without this passphrase the \
                                         file cannot be opened by anyone, including us."
                                    ))) },
                                    Err(e) => backup_msg.set(Some(e)),
                                }
                            },
                            "Save backup file"
                        }
                    }
                    if let Some(m) = backup_msg() {
                        div { class: "sval", "{m}" }
                    }
                }

                div { class: "setting",
                    div { class: "sname", "Restore on this device" }
                    div { class: "sval",
                        "Moving from another phone: pick the backup file and enter "
                        "its passphrase. This replaces the identity on this device, "
                        "so do it before you build a profile here."
                    }
                    label { class: "linkish restore-pick",
                        input {
                            r#type: "file",
                            onchange: move |evt: Event<FormData>| {
                                let files = evt.files();
                                spawn(async move {
                                    let Some(first) = files.into_iter().next() else { return };
                                    let Ok(bytes) = first.read_bytes().await else {
                                        backup_msg.set(Some("could not read that file".into()));
                                        return;
                                    };
                                    let phrase = pass.peek().clone();
                                    match Identity::from_backup_with(&bytes, &phrase) {
                                        Ok((id, extra)) => {
                                            restore_into_device(&id, &extra);
                                            backup_msg.set(Some(
                                                "Restored. Close and reopen the app.".into(),
                                            ));
                                        }
                                        // One message for both wrong-passphrase and
                                        // corrupt-file, because the code cannot tell
                                        // them apart -- AEAD failure is the same
                                        // either way -- and guessing would send
                                        // someone hunting the wrong problem.
                                        Err(_) => backup_msg.set(Some(
                                            "That passphrase did not open the file, or \
                                             the file is damaged."
                                                .into(),
                                        )),
                                    }
                                });
                            },
                        }
                        "Choose a backup file"
                    }
                }

                div { class: "setting",
                    div { class: "sname", "Your node" }
                    div { class: "sval",
                        "This phone is part of the network. It carries traffic for others "
                        "only while charging on Wi-Fi, and you can stop it any time from "
                        "the notification."
                    }
                }
                div { class: "setting",
                    div { class: "sname", "What this does not hide" }
                    div { class: "sval",
                        "Your photo is public to people nearby. Anything published can be "
                        "copied and cannot be fully deleted. Other people's stated "
                        "location may not be true."
                    }
                }
            }
        }
    }
}
