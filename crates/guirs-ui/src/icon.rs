//! The icon element and a small built in set.
//!
//! Icons behave like text: they take their size from `font-size` unless given
//! one, and their color from `color`, so an icon sitting next to a label
//! matches it without being told to.

use guirs_core::{Bounds, ElementId, GlobalElementId, Icon, Px, SharedString, Size};
use guirs_render::IconSprite;
use guirs_style::{ComputedStyle, StateFlags, Style, Styled};
use smallvec::SmallVec;

use crate::context::{content_box, Cx};
use crate::element::Element;
use crate::layout::LayoutId;

/// A vector icon.
pub struct IconElement {
    icon: Icon,
    element_type: SharedString,
    id: Option<ElementId>,
    classes: SmallVec<[SharedString; 4]>,
    style: Style,

    global_id: GlobalElementId,
    computed: ComputedStyle,
    node: Option<LayoutId>,
}

/// Draw an icon.
///
/// ```
/// # use guirs_core::color::rgb;
/// # use guirs_style::Styled;
/// # use guirs_ui::{icon::{icon, icons}, text::text, widgets::row};
/// icon(icons::close()).text_color(rgb(0xffffff));
/// row().child(icon(icons::search())).child(text("Search"));
/// ```
pub fn icon(icon: Icon) -> IconElement {
    IconElement::new(icon)
}

impl IconElement {
    pub fn new(icon: Icon) -> Self {
        IconElement {
            icon,
            element_type: SharedString::from("icon"),
            id: None,
            classes: SmallVec::new(),
            style: Style::new(),
            global_id: GlobalElementId::ROOT,
            computed: ComputedStyle::default(),
            node: None,
        }
    }

    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Add one or more classes, separated by whitespace.
    pub fn class(mut self, class: impl Into<SharedString>) -> Self {
        crate::div::push_classes(&mut self.classes, class.into());
        self
    }

    pub fn class_if(self, condition: bool, class: impl Into<SharedString>) -> Self {
        if condition {
            self.class(class)
        } else {
            self
        }
    }

    /// Report a different type name to the stylesheet.
    pub fn as_type(mut self, element_type: &'static str) -> Self {
        self.element_type = SharedString::from(element_type);
        self
    }

    /// The size an icon takes when nothing sizes it.
    ///
    /// Matching the font size is what lets an icon sit inline with a label and
    /// come out the same height without being measured by hand.
    fn intrinsic(&self) -> Size<Px> {
        Size::new(self.computed.font_size, self.computed.font_size)
    }
}

impl Styled for IconElement {
    #[inline]
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

impl Element for IconElement {
    fn request_layout(&mut self, cx: &mut Cx) -> LayoutId {
        self.global_id = cx.begin_element(
            self.id.as_ref(),
            &self.element_type,
            &self.classes,
            false,
            StateFlags::EMPTY,
        );
        self.computed = cx.compute_style(&self.style);
        cx.end_element();

        let node = cx.layout.add_fixed_node(&self.computed, self.intrinsic());
        self.node = Some(node);
        node
    }

    fn paint(&mut self, bounds: Bounds<Px>, cx: &mut Cx) {
        if !crate::context::is_visible(&self.computed) {
            return;
        }

        cx.push_opacity(self.computed.opacity);
        let opacity = cx.opacity();
        let content = content_box(bounds, &self.computed, cx.root_font_size);

        // Keep the artwork square inside whatever box layout produced, so an
        // icon in a stretched row does not come out oval.
        let side = content.size.width.min(content.size.height);
        if side > Px::ZERO {
            let square = Bounds::from_xywh(
                content.origin.x + (content.size.width - side) * 0.5,
                content.origin.y + (content.size.height - side) * 0.5,
                side,
                side,
            );
            cx.scene.push_icon(IconSprite {
                icon: self.icon.clone(),
                bounds: square,
                color: self.computed.color.fade(opacity),
            });
        }

        cx.register_hit(
            self.global_id,
            bounds,
            self.computed.cursor,
            false,
            false,
        );
        cx.pop_opacity();
    }
}

crate::impl_into_element!(IconElement);

/// A small set of interface icons.
///
/// Drawn on a 24 unit grid with a 2 unit stroke, the convention most icon sets
/// use, so they sit together evenly and match a 24 unit set dropped in
/// alongside them.
pub mod icons {
    use guirs_core::Icon;
    use std::sync::LazyLock;

    macro_rules! icon {
        ($(#[$doc:meta])* $name:ident, $path:expr) => {
            $(#[$doc])*
            pub fn $name() -> Icon {
                static ICON: LazyLock<Icon> = LazyLock::new(|| Icon::stroked($path, 24, 2.0));
                // Cloning an icon is a refcount bump, so building one per
                // element per frame costs nothing.
                ICON.clone()
            }
        };
    }

    // -- window controls --

    icon!(
        /// A cross, for closing.
        close,
        "M18 6 L6 18 M6 6 L18 18"
    );
    icon!(
        /// A single bar, for minimizing.
        minimize,
        "M5 12 H19"
    );
    icon!(
        /// An empty square, for maximizing.
        maximize,
        "M5 5 H19 V19 H5 Z"
    );
    icon!(
        /// Two offset squares, for restoring a maximized window.
        restore,
        "M8 8 H19 V19 H8 Z M5 16 V5 H16"
    );

    // -- navigation --

    icon!(
        /// Three bars.
        menu,
        "M4 7 H20 M4 12 H20 M4 17 H20"
    );
    icon!(chevron_down, "M6 9 L12 15 L18 9");
    icon!(chevron_up, "M6 15 L12 9 L18 15");
    icon!(chevron_right, "M9 6 L15 12 L9 18");
    icon!(chevron_left, "M15 6 L9 12 L15 18");
    icon!(arrow_up, "M12 20 V4 M5 11 L12 4 L19 11");
    icon!(arrow_down, "M12 4 V20 M5 13 L12 20 L19 13");

    icon!(
        /// A pane with a divider, for showing and hiding a sidebar.
        sidebar,
        "M6 4 H18 A3 3 0 0 1 21 7 V17 A3 3 0 0 1 18 20 H6 A3 3 0 0 1 3 17 V7 A3 3 0 0 1 6 4 Z \
         M9.5 4 V20 \
         M6.2 9 A0.35 0.35 0 1 0 6.2 9.01 \
         M6.2 12 A0.35 0.35 0 1 0 6.2 12.01"
    );

    icon!(
        /// A square with a pencil over it, for starting something new.
        compose,
        "M12.5 4 H7 A3 3 0 0 0 4 7 V17 A3 3 0 0 0 7 20 H17 A3 3 0 0 0 20 17 V11.5 \
         M18.2 3.3 A2.1 2.1 0 0 1 21.2 6.3 L12.4 15.1 L8.6 16 L9.5 12.2 Z"
    );

    icon!(
        /// A globe, for anything to do with the web.
        globe,
        "M12 3 A9 9 0 1 0 12 21 A9 9 0 1 0 12 3 Z M3 12 H21 \
         M12 3 A13 13 0 0 1 12 21 A13 13 0 0 1 12 3 Z"
    );

    icon!(
        /// Four squares, for a directory of things.
        grid,
        "M4 4 H10 V10 H4 Z M14 4 H20 V10 H14 Z M4 14 H10 V20 H4 Z M14 14 H20 V20 H14 Z"
    );

    icon!(
        /// Bars of differing height, for anything to do with a voice.
        waveform,
        "M5 10 V14 M9 6 V18 M12 3 V21 M15 6 V18 M19 10 V14"
    );

    icon!(
        /// A circle drawn in dashes, for something that will not be kept.
        dashed_circle,
        "M12 3 A9 9 0 0 1 17 5 M20 8 A9 9 0 0 1 21 12 M20 16 A9 9 0 0 1 17 19 \
         M12 21 A9 9 0 0 1 7 19 M4 16 A9 9 0 0 1 3 12 M4 8 A9 9 0 0 1 7 5"
    );

    icon!(
        /// A telescope, for looking into something properly.
        research,
        "M3.5 13.2 L13 7.4 L16.4 12.9 L6.9 18.7 Z \
         M14.6 5.6 L19.6 8.6 M10.6 16.6 L12 21 M9 21 H14.5"
    );

    icon!(
        /// A ring with a gap, standing in for an application's own mark.
        ///
        /// Deliberately generic. A real product's mark is that product's, and
        /// a widget set has no business shipping one.
        ring,
        "M12 4 A8 8 0 1 1 6 7"
    );

    icon!(
        /// A filled dot inside a ring.
        orb,
        "M12 3 A9 9 0 1 0 12 21 A9 9 0 1 0 12 3 Z M12 8 A4 4 0 1 0 12 16 A4 4 0 1 0 12 8 Z"
    );

    icon!(
        /// Three dots in a row.
        ellipsis,
        "M6 12 A0.6 0.6 0 1 0 6 12.01 M12 12 A0.6 0.6 0 1 0 12 12.01 \
         M18 12 A0.6 0.6 0 1 0 18 12.01"
    );

    // -- actions --

    icon!(plus, "M12 5 V19 M5 12 H19");
    icon!(check, "M4 12.5 L9.5 18 L20 6");
    icon!(
        /// A magnifier.
        search,
        "M11 4 A7 7 0 1 0 11 18 A7 7 0 1 0 11 4 Z M16.2 16.2 L21 21"
    );
    icon!(
        /// A tray with an arrow going in.
        upload,
        "M4 15 V19 A1 1 0 0 0 5 20 H19 A1 1 0 0 0 20 19 V15 M8 8 L12 4 L16 8 M12 4 V15"
    );
    icon!(
        /// A tray with an arrow coming out.
        download,
        "M4 15 V19 A1 1 0 0 0 5 20 H19 A1 1 0 0 0 20 19 V15 M8 11 L12 15 L16 11 M12 15 V4"
    );
    icon!(
        /// Two overlapping sheets.
        copy,
        "M9 9 H19 V19 H9 Z M5 15 V5 H15"
    );
    icon!(
        /// A bin.
        trash,
        "M4 7 H20 M9 7 V5 H15 V7 M6 7 L7 20 H17 L18 7 M10 11 V16 M14 11 V16"
    );
    icon!(
        /// A pencil.
        edit,
        "M4 20 L4.7 15.8 L16 4.5 A2 2 0 0 1 19.5 8 L8.2 19.3 Z"
    );
    icon!(
        /// A circular arrow.
        refresh,
        "M20 12 A8 8 0 1 1 17.2 6 M20 4 V10 H14"
    );

    // -- objects --

    icon!(
        /// A speech bubble.
        message,
        "M4 6 A2 2 0 0 1 6 4 H18 A2 2 0 0 1 20 6 V15 A2 2 0 0 1 18 17 H9 L4 21 Z"
    );
    icon!(
        /// A head and shoulders.
        user,
        "M12 4 A4 4 0 1 0 12 12 A4 4 0 1 0 12 4 Z M4 21 A8 8 0 0 1 20 21"
    );
    icon!(
        /// A cog.
        settings,
        "M12 8.5 A3.5 3.5 0 1 0 12 15.5 A3.5 3.5 0 1 0 12 8.5 Z \
         M12 2 V5 M12 19 V22 M22 12 H19 M5 12 H2 \
         M19.1 4.9 L17 7 M7 17 L4.9 19.1 M19.1 19.1 L17 17 M7 7 L4.9 4.9"
    );
    icon!(
        /// A paperclip.
        attach,
        "M20 11.5 L12 19.5 A5 5 0 0 1 5 12.5 L13.5 4 A3.5 3.5 0 0 1 18.4 9 L10 17.4 A1.8 1.8 0 0 1 7.5 14.9 L15 7.5"
    );
    icon!(
        /// A horizontal ellipsis, for an overflow menu.
        more,
        "M6 12 H6.01 M12 12 H12.01 M18 12 H18.01"
    );
    icon!(
        /// A five pointed star.
        star,
        "M12 3.5 L14.6 9.2 L20.8 10 L16.2 14.2 L17.4 20.3 L12 17.3 L6.6 20.3 L7.8 14.2 L3.2 10 L9.4 9.2 Z"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::Cx;
    use crate::div::div;
    use crate::element::IntoElement;
    use guirs_core::ScaleFactor;
    use guirs_style::StyleEngine;
    use guirs_text::TextSystem;

    fn frame(mut root: crate::AnyElement, styles: StyleEngine) -> Cx {
        let size = Size::new(Px(200.0), Px(200.0));
        let mut cx = Cx::new(TextSystem::new(), styles);
        cx.begin_frame(size, ScaleFactor(1.0), 0.0);
        let node = root.request_layout(&mut cx);
        cx.layout.compute(node, size, &mut cx.text);
        root.paint(cx.layout.bounds(node), &mut cx);
        cx.end_frame();
        cx
    }

    #[test]
    fn an_icon_emits_one_sprite() {
        let cx = frame(
            icon(icons::close()).into_any(),
            StyleEngine::default(),
        );
        assert_eq!(cx.scene.stats().sprites, 1);
    }

    #[test]
    fn an_icon_takes_its_size_from_the_font_size() {
        let cx = frame(
            div()
                .text_size(32.0)
                .child(icon(icons::menu()))
                .into_any(),
            StyleEngine::default(),
        );
        // The icon's own hit region is the box layout gave it.
        let icon_bounds = cx.hits.last().unwrap().bounds;
        assert_eq!(icon_bounds.size.width, Px(32.0));
        assert_eq!(icon_bounds.size.height, Px(32.0));
    }

    #[test]
    fn an_explicit_size_wins_over_the_font_size() {
        let cx = frame(
            div()
                .text_size(32.0)
                .child(icon(icons::menu()).size(Px(12.0)))
                .into_any(),
            StyleEngine::default(),
        );
        assert_eq!(cx.hits.last().unwrap().bounds.size.width, Px(12.0));
    }

    #[test]
    fn a_stylesheet_can_size_and_color_icons() {
        let cx = frame(
            icon(icons::search()).class("big").into_any(),
            StyleEngine::from_source("icon.big { width: 40px; height: 40px; color: #ff0000; }"),
        );
        assert_eq!(cx.hits[0].bounds.size.width, Px(40.0));
        assert_eq!(cx.scene.stats().sprites, 1);
    }

    #[test]
    fn a_transparent_icon_draws_nothing() {
        let cx = frame(
            icon(icons::close())
                .text_color(guirs_core::Rgba::TRANSPARENT)
                .into_any(),
            StyleEngine::default(),
        );
        assert_eq!(cx.scene.stats().sprites, 0);
    }

    #[test]
    fn every_built_in_icon_has_usable_path_data() {
        let all = [
            icons::close(),
            icons::minimize(),
            icons::maximize(),
            icons::restore(),
            icons::menu(),
            icons::chevron_down(),
            icons::chevron_up(),
            icons::chevron_right(),
            icons::chevron_left(),
            icons::arrow_up(),
            icons::arrow_down(),
            icons::plus(),
            icons::check(),
            icons::search(),
            icons::upload(),
            icons::download(),
            icons::copy(),
            icons::trash(),
            icons::edit(),
            icons::refresh(),
            icons::message(),
            icons::user(),
            icons::settings(),
            icons::attach(),
            icons::more(),
            icons::star(),
        ];

        for icon in &all {
            assert_eq!(icon.view_box(), 24, "icons share a 24 unit grid");
            assert!(icon.path().starts_with('M'), "a path starts with a move");
            assert!(
                icon.path().len() > 4,
                "suspiciously short path: {}",
                icon.path()
            );
        }

        // Every icon is a distinct picture.
        let mut keys: Vec<u64> = all.iter().map(|icon| icon.key()).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "two icons share a path");
    }

    #[test]
    fn asking_for_an_icon_twice_shares_one_allocation() {
        let a = icons::close();
        let b = icons::close();
        assert_eq!(a, b);
        assert_eq!(a.key(), b.key());
    }
}
