use freya::prelude::*;

/// The results pane while a query executes. Placeholder for now — a proper spinner + elapsed time
/// land with the runtime layer.
#[derive(PartialEq)]
pub struct Running;

impl Component for Running {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            .height(Size::flex(1.))
            .center()
            .child(label().text("Running query…").theme_color())
    }
}
