//! Shared renderer for a structured query error's *body* — the message, an
//! optional code frame (offending line + caret), and an optional hint. Used by
//! both the results-pane error view (`workspace::results_error`) and the
//! expandable error rows in the Events drawer (`drawer`), so the two stay in
//! step. Callers supply their own surrounding chrome (banner / box).

use dioxus::prelude::*;

use crate::query_error::QueryError;

/// Render `message → code frame → hint` for `err`, using the `.err-*` classes.
pub fn error_detail(err: &QueryError) -> Element {
    let hint = err.hint.clone().unwrap_or_default();
    let frame = err.frame.clone();
    let message = err.message.clone();
    rsx! {
        div { class: "err-msg mono", "{message}" }
        {
            match frame {
                Some(frame) => rsx! {
                    div { class: "err-frame",
                        div { class: "err-frame-row",
                            span { class: "err-ln mono", "{frame.line_no}" }
                            span { class: "err-code mono", "{frame.line_text}" }
                        }
                        div { class: "err-frame-row",
                            span { class: "err-ln mono", style: "color:transparent;", "{frame.line_no}" }
                            span { class: "err-caret mono", "{frame.caret_pad}{frame.caret}" }
                        }
                    }
                },
                None => rsx! {},
            }
        }
        if !hint.is_empty() {
            div { class: "err-hint", "{hint}" }
        }
    }
}
