//! LKNG grid UI.
//!
//! Rendering and event wiring only — every decision about *what* appears
//! lives in `lkng-app` where it is unit-tested. If a rule starts creeping
//! into this file, it belongs down there instead.
//!
//! This build runs against a seeded in-memory grid so the interface can be
//! developed and reviewed with no node, no network and no account. Wiring
//! it to `lkng-transport-freenet` swaps the data source and nothing else.

use dioxus::prelude::*;
use lkng_app::{Grid, Privacy, Session, Tile};
use lkng_identity::Identity;
use lkng_presence::CellState;

const CSS: Asset = asset!("/assets/lkng.css");

fn main() {
    dioxus::launch(App);
}

/// Build a demo grid: several strangers in and around one cell.
///
/// Every tile here is genuinely signed and genuinely verified by the same
/// code path the network uses — the only thing faked is delivery.
fn demo_grid() -> (Vec<Tile>, String) {
    let me = Session::new(Identity::from_seed([1; 32]), [2; 32], Privacy::Km1);
    let coverage = me
        .coverage(37.7749, -122.4194, 1_785_527_000)
        .expect("valid coordinates");
    let home = coverage.publish_target();
    let neighbour_cell = coverage.cells[1].clone();

    let people: [(u8, &str, u64, bool); 6] = [
        (10, "new in town, show me somewhere good", 900, true),
        (11, "coffee?", 850, true),
        (12, "up late, bad films", 800, true),
        (13, "here for the weekend", 700, true),
        (14, "next neighbourhood over", 990, false),
        (15, "walking the dog", 960, false),
    ];

    let mut grid = Grid::new();
    for (seed, headline, ts, same_cell) in people {
        let params = if same_cell {
            home.clone()
        } else {
            lkng_presence::CellParams {
                schema_v: lkng_app::SCHEMA_V,
                cell_id: neighbour_cell.as_str().to_string(),
                epoch: coverage.epochs[0],
            }
        };
        let them = Session::new(Identity::from_seed([seed; 32]), [seed; 32], Privacy::Km1);
        let tile = them
            .compose_tile(&params, headline, vec![seed; 48], ts)
            .expect("compose");
        let mut state = CellState::default();
        state.insert(tile);
        let mut bytes = Vec::new();
        ciborium::ser::into_writer(&state, &mut bytes).expect("cbor");
        grid.absorb(&me, &params, &bytes, &coverage.home)
            .expect("absorb");
    }
    let cell = coverage.home.as_str().to_string();
    (grid.tiles(), cell)
}

#[component]
fn App() -> Element {
    let (tiles, cell) = use_hook(demo_grid);
    let mut blocked = use_signal(Vec::<[u8; 32]>::new);
    let mut selected = use_signal(|| None::<Tile>);

    let visible: Vec<Tile> = tiles
        .iter()
        .filter(|t| !blocked.read().contains(&t.pseudonym))
        .cloned()
        .collect();

    rsx! {
        document::Link { rel: "stylesheet", href: CSS }
        header { class: "bar",
            div { class: "brand", "LKNG" }
            div { class: "cell",
                span { class: "dot" }
                "{cell}"
            }
        }

        main {
            p { class: "note",
                "Everyone nearby, no distances, no company in the middle. "
                "Tiles below are cryptographically verified."
            }

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

/// Placeholder tile art derived from the pseudonym, so the grid looks like
/// a grid before photo support lands. Deterministic per pseudonym, and it
/// changes when the pseudonym rotates — which is the honest behaviour.
fn swatch(pseudonym: &[u8; 32]) -> String {
    let h = u16::from(pseudonym[0]) * 360 / 255;
    let h2 = (h + 40) % 360;
    format!("linear-gradient(135deg, hsl({h} 55% 42%), hsl({h2} 45% 26%))")
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes[..6].iter().map(|b| format!("{b:02x}")).collect()
}
