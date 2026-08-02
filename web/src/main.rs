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
pub const BUILD_MARKER: &str = "b32784";

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
#[derive(Clone, Copy, PartialEq)]
enum Fix {
    /// From the device, via the coarse-location bridge.
    Device,
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
fn coverage_for(s: &Session) -> (Coverage, Fix) {
    let (pos, fix) = match device_position() {
        Some(p) => (p, Fix::Device),
        None if location_bridge_present() => ((37.7749, -122.4194), Fix::Waiting),
        None => ((37.7749, -122.4194), Fix::None),
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

#[component]
fn App() -> Element {
    let session = use_hook(me);
    let (cov, fix) = use_hook(|| coverage_for(&me()));
    let node = use_hook(Node::connect);

    // Once the socket opens, ask for every cell we watch and subscribe, so
    // later arrivals are pushed rather than polled.
    let mut requested = use_signal(|| false);
    {
        let node = node.clone();
        let cov = cov.clone();
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

    let status = node.status.borrow().clone();
    let (found, rejected) = live_tiles(&session, &cov, &node);
    let live = !found.is_empty();
    let tiles = if live { found } else { demo_tiles(&session, &cov) };

    let mut tab = use_signal(|| Tab::Browse);
    let mut menu = use_signal(|| false);
    let draft = use_signal(load_draft);
    let mut blocked = use_signal(Vec::<[u8; 32]>::new);
    let mut selected = use_signal(|| None::<Tile>);
    let mut open_thread = use_signal(|| None::<[u8; 32]>);
    let mut compose = use_signal(String::new);
    let mut send_error = use_signal(|| None::<String>);
    let mut tapped = use_signal(|| false);
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
        chat::merge_threads(all)
    };

    // A thread with nothing but taps belongs on the Taps screen, not in
    // Messages: it is a signal, not a conversation, and mixing them makes
    // the message list look full of things nobody actually said.
    let (taps, threads): (Vec<Thread>, Vec<Thread>) = threads
        .into_iter()
        .partition(|t| t.messages.iter().all(|m| m.kind == chat::Kind::Tap));

    let note = match (&status, live) {
        (Status::Connected, true) => {
            "Live from the network. Everyone nearby, no distances, no company in the middle."
                .to_string()
        }
        (Status::Connected, false) => match fix {
            Fix::Device => "Connected — nobody in your area yet.".to_string(),
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
                if fix != Fix::Device {
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
                        div { class: "thumb sm", style: "background: {swatch(&th.peer)}" }
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
                        div { class: "thumb sm", style: "background: {swatch(&th.peer)}" }
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
                h2 { class: "screen-h", "Albums" }
                div { class: "empty",
                    "Albums aren't built yet."
                    br {}
                    span { class: "hint",
                        "When they are, an album will be shared with named people "
                        "rather than published — a private photo on a public network "
                        "cannot be un-shared, so it must never be public in the first place."
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
                    div { class: "sheet-thumb", style: "background: {swatch(&t.pseudonym)}" }
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
    node.seed(INBOX_WASM, &params_bytes, cbor(&empty));

    // The delta is a state carrying exactly this envelope; the contract
    // merges it in. Deltas are self-contained for the same reason presence
    // records are (River #145) — a delta referring to something the peer has
    // never seen is dropped without a word.
    let mut delta = lkng_inbox::InboxState::default();
    delta.insert(env);
    node.update(key, cbor(&delta));
    Ok(())
}

#[component]
fn TileCard(tile: Tile, onopen: EventHandler<Tile>) -> Element {
    let t = tile.clone();
    rsx! {
        button {
            class: "tile",
            onclick: move |_| onopen.call(t.clone()),
            div { class: "thumb", style: "background: {swatch(&tile.pseudonym)}" }
            div { class: "overlay",
                div { class: "headline", "{tile.headline}" }
                if !tile.same_cell {
                    div { class: "badge", "next area" }
                }
            }
        }
    }
}

/// Placeholder tile art derived from the pseudonym, so the grid reads as a
/// grid before photo support lands. Deterministic per pseudonym, and it
/// changes when the pseudonym rotates — which is the honest behaviour.
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
