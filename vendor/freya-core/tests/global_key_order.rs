use freya_core::prelude::*;
use freya_testing::prelude::*;

/// Same-name global listeners fire in document (pre-order) order — even when a listener
/// mounts later than its siblings — and `prevent_default()` consumes the event so the
/// remaining global listeners never see that press.
#[test]
pub fn global_key_listeners_document_order_and_consumption() {
    fn app() -> impl IntoElement {
        let mut log = use_state(Vec::<&'static str>::new);
        let mut armed = use_state(|| false);

        rect()
            // Mounted only after the first press: registered last, but positioned first
            // in the tree — document order must still fire it first. It consumes.
            .maybe(armed(), |el| {
                el.child(
                    rect().on_global_key_down(move |e: Event<KeyboardEventData>| {
                        log.write().push("early");
                        e.prevent_default();
                    }),
                )
            })
            .child(
                rect().on_global_key_down(move |_: Event<KeyboardEventData>| {
                    log.write().push("mid");
                    armed.set(true);
                }),
            )
            .child(
                rect().on_global_key_down(move |_: Event<KeyboardEventData>| {
                    log.write().push("tail")
                }),
            )
            .child(label().text(log.read().join(",")))
    }

    let mut test = launch_test(app);
    test.sync_and_update();

    // First press: the early listener is not mounted yet; mid and tail both fire, in
    // document order.
    test.press_key(Key::Named(NamedKey::Enter));
    let label = test
        .find(|_, element| Label::try_downcast(element).filter(|l| l.text.as_ref() == "mid,tail"));
    assert!(label.is_some(), "expected mid,tail after the first press");

    // Second press: early — first in document order despite being registered last —
    // fires and consumes, so mid and tail never see the event.
    test.press_key(Key::Named(NamedKey::Enter));
    let label = test.find(|_, element| {
        Label::try_downcast(element).filter(|l| l.text.as_ref() == "mid,tail,early")
    });
    assert!(label.is_some(), "expected early to fire first and consume");
}
