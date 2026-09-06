#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum EventName {
    // Platform Mouse
    MouseUp,
    MouseDown,
    MouseMove,

    // Platform Mouse or Touch
    PointerPress,
    PointerDown,
    PointerMove,
    PointerEnter,
    PointerLeave,
    PointerOver,
    PointerOut,

    // Platform Keyboard
    KeyDown,
    KeyUp,

    // Platform Touch
    TouchCancel,
    TouchStart,
    TouchMove,
    TouchEnd,

    GlobalPointerMove,
    GlobalPointerPress,
    GlobalPointerDown,

    GlobalKeyDown,
    GlobalKeyUp,

    GlobalFileHover,
    GlobalFileHoverCancelled,

    CaptureGlobalPointerMove,
    CaptureGlobalPointerPress,

    Wheel,

    Sized,

    Styled,

    FileDrop,

    ImePreedit,
}

use std::collections::HashSet;

use ragnarok::NameOfEvent as _;

impl PartialOrd for EventName {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EventName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority()
            .cmp(&other.priority())
            .then_with(|| (*self as u8).cmp(&(*other as u8)))
    }
}

impl EventName {
    /// Emission priority class: lower fires first. Capture events preempt everything,
    /// leave events precede enter events (exclusive leave first, over before enter),
    /// non-capture globals fire last. Same-class events tie-break by variant so the
    /// order is a consistent total order (`sort_unstable` requires it); equality holds
    /// only for the same variant.
    fn priority(&self) -> u8 {
        if self.is_capture() {
            0
        } else if self.is_exclusive_left() {
            1
        } else if self.is_left() {
            2
        } else if self.is_non_exclusive_enter() {
            3
        } else if self.is_global() {
            5
        } else {
            4
        }
    }

    /// Check if this even captures others or not
    pub fn is_capture(&self) -> bool {
        matches!(
            &self,
            Self::CaptureGlobalPointerMove | Self::CaptureGlobalPointerPress
        )
    }

    /// Check if this is a global pointer event
    pub fn is_global_pointer(&self) -> bool {
        matches!(
            self,
            Self::GlobalPointerMove
                | Self::GlobalPointerPress
                | Self::GlobalPointerDown
                | Self::CaptureGlobalPointerMove
                | Self::CaptureGlobalPointerPress
        )
    }

    pub fn is_left(&self) -> bool {
        matches!(&self, Self::PointerLeave | Self::PointerOut)
    }

    pub fn is_exclusive_left(&self) -> bool {
        matches!(&self, Self::PointerLeave)
    }

    pub fn is_non_exclusive_enter(&self) -> bool {
        matches!(&self, Self::PointerOver)
    }

    pub fn is_down(&self) -> bool {
        matches!(self, Self::PointerDown)
    }

    pub fn is_pointer_move(&self) -> bool {
        matches!(self, Self::PointerMove)
    }

    pub fn is_press(&self) -> bool {
        matches!(self, Self::PointerPress)
    }
}

impl ragnarok::NameOfEvent for EventName {
    fn get_global_events(&self) -> HashSet<Self> {
        match self {
            Self::MouseUp | Self::TouchEnd => {
                HashSet::from([Self::GlobalPointerPress, Self::CaptureGlobalPointerPress])
            }
            Self::MouseDown | Self::TouchStart => HashSet::from([Self::GlobalPointerDown]),
            Self::MouseMove | Self::TouchMove => {
                HashSet::from([Self::GlobalPointerMove, Self::CaptureGlobalPointerMove])
            }

            Self::KeyDown => HashSet::from([Self::GlobalKeyDown]),
            Self::KeyUp => HashSet::from([Self::GlobalKeyUp]),

            Self::GlobalFileHover => HashSet::from([Self::GlobalFileHover]),
            Self::GlobalFileHoverCancelled => HashSet::from([Self::GlobalFileHoverCancelled]),
            _ => HashSet::new(),
        }
    }

    fn get_derived_events(&self) -> HashSet<Self> {
        let mut events = HashSet::new();

        events.insert(*self);

        match self {
            Self::MouseMove | Self::TouchMove => {
                events.insert(Self::PointerMove);
                events.insert(Self::PointerEnter);
                events.insert(Self::PointerOver);
            }
            Self::MouseDown | Self::TouchStart => {
                events.insert(Self::PointerDown);
            }
            Self::MouseUp | Self::TouchEnd => {
                events.insert(Self::PointerPress);
            }
            Self::PointerOut => {
                events.insert(Self::PointerLeave);
            }
            _ => {}
        }

        events
    }

    fn get_cancellable_events(&self) -> HashSet<Self> {
        let mut events = HashSet::new();

        events.insert(*self);

        match self {
            Self::KeyDown => {
                events.insert(Self::GlobalKeyDown);
            }
            Self::KeyUp => {
                events.insert(Self::GlobalKeyUp);
            }
            Self::MouseUp | Self::TouchEnd => {
                events.extend([Self::PointerPress, Self::GlobalPointerPress])
            }
            Self::PointerPress => events.extend([Self::MouseUp, Self::GlobalPointerPress]),
            Self::MouseDown | Self::TouchStart => {
                events.extend([Self::PointerDown, Self::GlobalPointerDown])
            }
            Self::PointerDown => events.extend([Self::MouseDown, Self::GlobalPointerDown]),
            Self::CaptureGlobalPointerMove => {
                events.extend([
                    Self::MouseMove,
                    Self::TouchMove,
                    Self::PointerMove,
                    Self::PointerEnter,
                    Self::PointerOver,
                    Self::GlobalPointerMove,
                ]);
            }
            Self::CaptureGlobalPointerPress => {
                events.extend([
                    Self::MouseUp,
                    Self::TouchEnd,
                    Self::PointerPress,
                    Self::GlobalPointerPress,
                ]);
            }

            _ => {}
        }

        events
    }

    fn is_global(&self) -> bool {
        matches!(
            self,
            Self::GlobalKeyDown
                | Self::GlobalKeyUp
                | Self::GlobalPointerPress
                | Self::GlobalPointerDown
                | Self::GlobalPointerMove
                | Self::GlobalFileHover
                | Self::GlobalFileHoverCancelled
        )
    }

    fn is_moved(&self) -> bool {
        matches!(
            &self,
            Self::MouseMove
                | Self::TouchMove
                | Self::PointerMove
                | Self::CaptureGlobalPointerMove
                | Self::GlobalPointerMove
        )
    }

    fn does_bubble(&self) -> bool {
        !self.is_moved()
            && !self.is_enter()
            && !self.is_left()
            && !self.is_global()
            && !self.is_capture()
    }

    fn is_emitted_once(&self) -> bool {
        self.does_bubble() || self.is_exclusive_enter() || self.is_exclusive_leave()
    }

    fn does_go_through_solid(&self) -> bool {
        // TODO
        false
    }

    fn is_enter(&self) -> bool {
        matches!(&self, Self::PointerEnter | Self::PointerOver)
    }

    fn is_pressed(&self) -> bool {
        matches!(self, Self::MouseDown | Self::PointerDown | Self::TouchStart)
    }

    fn is_released(&self) -> bool {
        matches!(&self, Self::PointerPress)
    }

    fn is_exclusive_enter(&self) -> bool {
        matches!(&self, Self::PointerEnter)
    }

    fn is_exclusive_leave(&self) -> bool {
        matches!(&self, Self::PointerLeave)
    }

    fn new_leave() -> Self {
        Self::PointerOut
    }

    fn new_exclusive_leave() -> Self {
        Self::PointerLeave
    }

    fn new_exclusive_enter() -> Self {
        Self::PointerEnter
    }
}

#[cfg(test)]
mod test {
    use super::EventName;

    const ALL: &[EventName] = &[
        EventName::MouseUp,
        EventName::MouseDown,
        EventName::MouseMove,
        EventName::PointerPress,
        EventName::PointerDown,
        EventName::PointerMove,
        EventName::PointerEnter,
        EventName::PointerLeave,
        EventName::PointerOver,
        EventName::PointerOut,
        EventName::KeyDown,
        EventName::KeyUp,
        EventName::TouchCancel,
        EventName::TouchStart,
        EventName::TouchMove,
        EventName::TouchEnd,
        EventName::GlobalPointerMove,
        EventName::GlobalPointerPress,
        EventName::GlobalPointerDown,
        EventName::GlobalKeyDown,
        EventName::GlobalKeyUp,
        EventName::GlobalFileHover,
        EventName::GlobalFileHoverCancelled,
        EventName::CaptureGlobalPointerMove,
        EventName::CaptureGlobalPointerPress,
        EventName::Wheel,
        EventName::Sized,
        EventName::Styled,
        EventName::FileDrop,
        EventName::ImePreedit,
    ];

    /// `sort_unstable` requires a consistent total order; the old comparator returned
    /// `Greater` for any global-vs-global pair, making same-name-class order arbitrary.
    #[test]
    fn ord_is_a_consistent_total_order() {
        for a in ALL {
            assert_eq!(a.cmp(a), std::cmp::Ordering::Equal);
            for b in ALL {
                // Antisymmetry, and equality only for the identical variant.
                assert_eq!(a.cmp(b), b.cmp(a).reverse(), "{a:?} vs {b:?}");
                if a != b {
                    assert_ne!(a.cmp(b), std::cmp::Ordering::Equal, "{a:?} vs {b:?}");
                }
                // Transitivity.
                for c in ALL {
                    if a.cmp(b) == b.cmp(c) {
                        assert_eq!(a.cmp(c), a.cmp(b), "{a:?} {b:?} {c:?}");
                    }
                }
            }
        }
    }

    /// The priority classes the rest of the event pipeline relies on.
    #[test]
    fn ord_priority_classes() {
        use ragnarok::NameOfEvent as _;
        // Capture first, globals last, focused key events before their global variants.
        assert!(EventName::CaptureGlobalPointerPress < EventName::KeyDown);
        assert!(EventName::KeyDown < EventName::GlobalKeyDown);
        assert!(EventName::PointerLeave < EventName::PointerOut);
        assert!(EventName::PointerOver < EventName::PointerEnter);
        for e in ALL {
            if e.is_global() && !e.is_capture() {
                assert!(
                    EventName::KeyDown < *e,
                    "{e:?} should sort after platform events"
                );
            }
        }
    }
}
