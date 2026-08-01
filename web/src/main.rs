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

mod net;

use dioxus::prelude::*;
use lkng_app::{Coverage, Grid, Privacy, Session, Tile, SCHEMA_V};
use lkng_identity::Identity;
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

/// Compiled presence-cell contract, embedded so the client can seed a cell
/// it does not host. Without the code travelling with the PUT there is no
/// way to write to a shared cell at all.
const CELL_WASM: &[u8] = include_bytes!(
    "../../contracts/presence-cell/target/wasm32-unknown-unknown/release/presence_cell.wasm"
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
/// # Known gap: this seed is in localStorage, not the Keystore
///
/// The seed is generated from the platform CSPRNG and is unique per
/// install — which is the part that actually matters, because a shared
/// seed would make every user the same person. But it is persisted in
/// `localStorage`, and PLAN.md is explicit that private keys belong in the
/// Android Keystore behind the identity delegate, never in web storage.
///
/// That work needs a JS bridge to the Kotlin shell and is not done. Until
/// it is, anything with code execution in this WebView can read the seed.
/// Recorded here rather than in a tracker because the person who removes
/// this comment is the person who has to have fixed it.
fn load_or_create(key: &str) -> [u8; 32] {
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());

    if let Some(s) = &storage {
        if let Ok(Some(hex)) = s.get_item(key) {
            if let Some(bytes) = decode_hex(&hex) {
                return bytes;
            }
        }
    }
    let mut seed = [0u8; 32];
    getrandom_03::fill(&mut seed).expect("platform CSPRNG");
    if let Some(s) = &storage {
        let _ = s.set_item(key, &encode_hex(&seed));
    }
    seed
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

fn coverage_for(s: &Session) -> Coverage {
    s.coverage(37.7749, -122.4194, 1_785_527_000)
        .expect("valid demo coordinates")
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

#[component]
fn App() -> Element {
    let session = use_hook(me);
    let cov = use_hook(|| coverage_for(&me()));
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

    let mut blocked = use_signal(Vec::<[u8; 32]>::new);
    let mut selected = use_signal(|| None::<Tile>);
    let visible: Vec<Tile> = tiles
        .into_iter()
        .filter(|t| !blocked.read().contains(&t.pseudonym))
        .collect();

    let note = match (&status, live) {
        (Status::Connected, true) => {
            "Live from the network. Everyone nearby, no distances, no company in the middle."
                .to_string()
        }
        (Status::Connected, false) => {
            "Connected — nobody in your cells yet. Showing sample tiles.".to_string()
        }
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
            div { class: "brand", "LKNG" }
            div { class: "cell",
                span { class: if live { "dot" } else { "dot off" } }
                "{cov.home.as_str()}"
            }
        }

        main {
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
                        button { class: "primary", "Say hello" }
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

        footer { class: "foot",
            "{visible.len()} nearby · location is coarse and jittered on your device"
        }
    }
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
