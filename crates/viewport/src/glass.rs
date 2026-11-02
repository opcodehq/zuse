//! Floating menu layers share their blur/tint/content paint order across hosts.
use gpui::{
    AnyElement, App, Bounds, Corners, Element, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window, px,
};
pub fn menu(child: impl IntoElement) -> GlassMenu {
    menu_with_radius(child, 10.)
}
pub fn menu_with_radius(child: impl IntoElement, radius: f32) -> GlassMenu {
    GlassMenu {
        child: child.into_any_element(),
        radius,
        blur: 20.,
        always: false,
    }
}
pub fn backdrop(child: impl IntoElement) -> GlassMenu {
    GlassMenu {
        child: child.into_any_element(),
        radius: 0.,
        blur: 8.,
        always: true,
    }
}
pub struct GlassMenu {
    child: AnyElement,
    radius: f32,
    blur: f32,
    always: bool,
}

impl Element for GlassMenu {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let blur = self.always || !crate::palette::is_light();
        if blur {
            window.paint_layer(bounds, |window| {
                window.paint_backdrop_blur(bounds, Corners::all(px(self.radius)), px(self.blur));
                self.child.paint(window, cx);
            });
        } else {
            self.child.paint(window, cx);
        }
    }
}

impl IntoElement for GlassMenu {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
