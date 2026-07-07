//! Inline SVG icons (stroke style, `currentColor`), matching the prototype.

use dioxus::prelude::*;

fn stroke(sz: u32, w: &str, children: Element) -> Element {
    rsx! {
        svg {
            width: "{sz}", height: "{sz}", "viewBox": "0 0 24 24", fill: "none",
            stroke: "currentColor", "stroke-width": "{w}",
            "stroke-linecap": "round", "stroke-linejoin": "round",
            {children}
        }
    }
}

/// The Strata brand mark — uneven sedimentary layers in the blue ramp, on a dark
/// square. Fills a rounded container (which supplies the corner radius).
pub fn strata_logo(sz: u32) -> Element {
    rsx! {
        svg { width: "{sz}", height: "{sz}", "viewBox": "0 0 24 24",
            rect { x: "-1", y: "-1", width: "26", height: "26", fill: "#0b1017" }
            polygon { points: "-1,1.92 25,-4.8 25,0.5 -1,7.22", fill: "#1a4a6e" }
            polygon { points: "-1,6.72 25,0 25,5.3 -1,12.02", fill: "#2b7fd0" }
            polygon { points: "-1,11.52 25,4.8 25,10.1 -1,16.82", fill: "#4cc6ff" }
            polygon { points: "-1,16.32 25,9.6 25,14.9 -1,21.62", fill: "#8fe0ff" }
        }
    }
}

pub fn folder(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! { path { d: "M3 7a2 2 0 0 1 2-2h4l2 2h6a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" } },
    )
}
/// Pushpin (B11 — pin a project to the top of the launcher).
pub fn pin(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! {
            line { x1: "12", y1: "17", x2: "12", y2: "22" }
            path { d: "M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z" }
        },
    )
}
/// External-link box (B11 — open a project in a new window).
pub fn external(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! {
            path { d: "M15 3h6v6" }
            path { d: "M10 14 21 3" }
            path { d: "M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" }
        },
    )
}
pub fn cube_lines(sz: u32) -> Element {
    stroke(
        sz,
        "1.6",
        rsx! {
            path { d: "m12 3 9 4.5-9 4.5-9-4.5z" }
            path { d: "m3 12 9 4.5 9-4.5M3 16.5 12 21l9-4.5" }
        },
    )
}
pub fn search(sz: u32) -> Element {
    stroke(
        sz,
        "1.8",
        rsx! { circle { cx: "11", cy: "11", r: "7" } path { d: "m20 20-3.5-3.5" } },
    )
}
pub fn plus(sz: u32) -> Element {
    stroke(sz, "2.2", rsx! { path { d: "M12 5v14M5 12h14" } })
}
pub fn minus(sz: u32) -> Element {
    stroke(sz, "2", rsx! { path { d: "M5 12h14" } })
}
pub fn close(sz: u32) -> Element {
    stroke(sz, "2", rsx! { path { d: "M6 6l12 12M18 6L6 18" } })
}
pub fn chevron_down(sz: u32) -> Element {
    stroke(sz, "2.2", rsx! { path { d: "m6 9 6 6 6-6" } })
}
pub fn chevron_right(sz: u32) -> Element {
    stroke(sz, "2.2", rsx! { path { d: "m9 6 6 6-6 6" } })
}
pub fn gear(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! {
            circle { cx: "12", cy: "12", r: "3" }
            path { d: "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" }
        },
    )
}
pub fn clock(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! { circle { cx: "12", cy: "12", r: "9" } path { d: "M12 7v5l3 2" } },
    )
}
pub fn format(sz: u32) -> Element {
    stroke(sz, "1.8", rsx! { path { d: "M4 6h16M4 12h10M4 18h13" } })
}
pub fn save(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! {
            path { d: "M6 3h10l4 4v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z" }
            path { d: "M8 3v5h7V3M8 21v-7h8v7" }
        },
    )
}
pub fn palette(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! {
            circle { cx: "13.5", cy: "6.5", r: ".8", fill: "currentColor" }
            circle { cx: "17", cy: "11", r: ".8", fill: "currentColor" }
            circle { cx: "8", cy: "7", r: ".8", fill: "currentColor" }
            circle { cx: "6.5", cy: "12", r: ".8", fill: "currentColor" }
            path { d: "M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.93 0 1.65-.75 1.65-1.69 0-.44-.18-.83-.44-1.12-.29-.29-.44-.65-.44-1.13a1.64 1.64 0 0 1 1.67-1.67h2c3.05 0 5.56-2.5 5.56-5.55C22 6.09 17.5 2 12 2z" }
        },
    )
}
pub fn grid(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! {
            rect { x: "3", y: "3", width: "7", height: "7", rx: "1" }
            rect { x: "14", y: "3", width: "7", height: "7", rx: "1" }
            rect { x: "3", y: "14", width: "7", height: "7", rx: "1" }
            rect { x: "14", y: "14", width: "7", height: "7", rx: "1" }
        },
    )
}
pub fn sliders(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! { path { d: "M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6" } },
    )
}
pub fn keyboard(sz: u32) -> Element {
    stroke(
        sz,
        "1.6",
        rsx! {
            rect { x: "2", y: "6", width: "20", height: "12", rx: "2" }
            path { d: "M6 10h.01M10 10h.01M14 10h.01M18 10h.01M8 14h8" }
        },
    )
}
pub fn info(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! { circle { cx: "12", cy: "12", r: "9" } path { d: "M12 11v5" } path { d: "M12 7.6v.01" } },
    )
}
pub fn download(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! { path { d: "M12 3v12M8 11l4 4 4-4M5 21h14" } },
    )
}
pub fn eye(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! { path { d: "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z" } circle { cx: "12", cy: "12", r: "2.5" } },
    )
}
pub fn pencil(sz: u32) -> Element {
    stroke(
        sz,
        "1.8",
        rsx! { path { d: "M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z" } },
    )
}
pub fn play(sz: u32) -> Element {
    rsx! { svg { width: "{sz}", height: "{sz}", "viewBox": "0 0 24 24", fill: "currentColor", path { d: "M8 5v14l11-7z" } } }
}
pub fn collapse_left(sz: u32) -> Element {
    stroke(
        sz,
        "1.9",
        rsx! { path { d: "m11 7-5 5 5 5M18 7l-5 5 5 5" } },
    )
}
pub fn expand_right(sz: u32) -> Element {
    stroke(sz, "1.9", rsx! { path { d: "m13 7 5 5-5 5M6 7l5 5-5 5" } })
}
pub fn branch(sz: u32) -> Element {
    stroke(
        sz,
        "1.9",
        rsx! { path { d: "M6 3v12a3 3 0 0 0 3 3h9M6 9h12" } },
    )
}
pub fn check(sz: u32) -> Element {
    stroke(sz, "2", rsx! { path { d: "M20 6 9 17l-5-5" } })
}
pub fn alert(sz: u32) -> Element {
    stroke(
        sz,
        "2",
        rsx! { path { d: "M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" } },
    )
}
pub fn table(sz: u32) -> Element {
    stroke(
        sz,
        "1.7",
        rsx! { rect { x: "3", y: "4", width: "18", height: "16", rx: "2" } path { d: "M3 9h18M3 14h18M9 4v16" } },
    )
}
/// Filled-outline circle with an exclamation — the results-pane error banner.
pub fn err_circle(sz: u32) -> Element {
    stroke(
        sz,
        "1.9",
        rsx! { circle { cx: "12", cy: "12", r: "9" } path { d: "M12 8v5M12 16h.01" } },
    )
}
/// Three horizontal rules with a divider — the "no results yet" empty state.
pub fn rows(sz: u32) -> Element {
    stroke(
        sz,
        "1.6",
        rsx! { path { d: "M3 5h18M3 12h18M3 19h18" } path { d: "M9 5v14" } },
    )
}
/// Spinning arc (needs the `.ps-spin` keyframe in main.css) — the running state.
pub fn spinner(sz: u32) -> Element {
    rsx! {
        svg {
            width: "{sz}", height: "{sz}", "viewBox": "0 0 24 24", fill: "none",
            stroke: "currentColor", "stroke-width": "2.4", "stroke-linecap": "round",
            class: "ps-spin",
            path { d: "M21 12a9 9 0 1 1-6.2-8.6" }
        }
    }
}
pub fn first(sz: u32) -> Element {
    stroke(sz, "1.9", rsx! { path { d: "M17 6l-6 6 6 6M8 6v12" } })
}
pub fn prev(sz: u32) -> Element {
    stroke(sz, "1.9", rsx! { path { d: "M15 6l-6 6 6 6" } })
}
pub fn next(sz: u32) -> Element {
    stroke(sz, "1.9", rsx! { path { d: "M9 6l6 6-6 6" } })
}
pub fn last(sz: u32) -> Element {
    stroke(sz, "1.9", rsx! { path { d: "M7 6l6 6-6 6M16 6v12" } })
}
pub fn trash(sz: u32) -> Element {
    stroke(
        sz,
        "1.8",
        rsx! {
            path { d: "M3 6h18M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" }
        },
    )
}
/// Vertical ellipsis (⋮) — the catalog row-action / overflow menu trigger.
pub fn dots(sz: u32) -> Element {
    rsx! {
        svg { width: "{sz}", height: "{sz}", "viewBox": "0 0 24 24", fill: "currentColor",
            circle { cx: "12", cy: "5", r: "1.6" }
            circle { cx: "12", cy: "12", r: "1.6" }
            circle { cx: "12", cy: "19", r: "1.6" }
        }
    }
}
pub fn database(sz: u32) -> Element {
    stroke(
        sz,
        "1.6",
        rsx! {
            ellipse { cx: "12", cy: "5", rx: "8", ry: "3" }
            path { d: "M4 5v6c0 1.66 3.58 3 8 3s8-1.34 8-3V5M4 11v6c0 1.66 3.58 3 8 3s8-1.34 8-3v-6" }
        },
    )
}
pub fn reopen(sz: u32) -> Element {
    stroke(
        sz,
        "1.8",
        rsx! {
            path { d: "M9 14l-4-4 4-4" }
            path { d: "M5 10h11a4 4 0 0 1 0 8h-1" }
        },
    )
}
pub fn brackets(sz: u32) -> Element {
    stroke(sz, "1.7", rsx! { path { d: "m8 8-4 4 4 4M16 8l4 4-4 4" } })
}
