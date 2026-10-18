//! A popup menu anchored at a point (right-click menus and button drop-downs).

use std::rc::Rc;

use gpui::{
    AnyElement, App, FontWeight, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Point,
    RenderOnce, SharedString, StatefulInteractiveElement, Styled, Window, anchored, deferred, div,
    prelude::FluentBuilder, px, svg,
};

use crate::ui::icons::Icon;
use crate::ui::theme::Theme;

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

enum Entry {
    Item { label: SharedString, icon: Option<Icon>, danger: bool, disabled: bool, handler: Handler },
    Separator,
    Header(SharedString),
}

/// Built fresh on each render while the owner keeps it open; `on_dismiss` must close it.
#[derive(IntoElement)]
pub struct Menu {
    id: SharedString,
    position: Point<Pixels>,
    entries: Vec<Entry>,
    on_dismiss: Handler,
    width: f32,
}

impl Menu {
    pub fn new(
        id: impl Into<SharedString>,
        position: Point<Pixels>,
        on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self { id: id.into(), position, entries: Vec::new(), on_dismiss: Rc::new(on_dismiss), width: 200.0 }
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn item(
        self,
        label: impl Into<SharedString>,
        icon: Option<Icon>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.entry(label, icon, false, false, handler)
    }

    /// An item drawn in a warning color (removing, clearing).
    pub fn danger_item(
        self,
        label: impl Into<SharedString>,
        icon: Option<Icon>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.entry(label, icon, true, false, handler)
    }

    pub fn disabled_item(self, label: impl Into<SharedString>, icon: Option<Icon>) -> Self {
        self.entry(label, icon, false, true, |_, _| {})
    }

    pub fn separator(mut self) -> Self {
        if !self.entries.is_empty() && !matches!(self.entries.last(), Some(Entry::Separator)) {
            self.entries.push(Entry::Separator);
        }
        self
    }

    pub fn header(mut self, text: impl Into<SharedString>) -> Self {
        self.entries.push(Entry::Header(text.into()));
        self
    }

    fn entry(
        mut self,
        label: impl Into<SharedString>,
        icon: Option<Icon>,
        danger: bool,
        disabled: bool,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.entries.push(Entry::Item {
            label: label.into(),
            icon,
            danger,
            disabled,
            handler: Rc::new(handler),
        });
        self
    }
}

impl RenderOnce for Menu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let danger_color = gpui::hsla(0.0, 0.72, if theme.dark { 0.68 } else { 0.5 }, 1.0);
        let on_dismiss = self.on_dismiss.clone();
        let entries: Vec<AnyElement> = self
            .entries
            .into_iter()
            .enumerate()
            .map(|(index, entry)| match entry {
                Entry::Separator => {
                    div().h(px(1.0)).mx(px(8.0)).my(px(4.0)).bg(theme.surface_panel_border).into_any_element()
                }
                Entry::Header(text) => div()
                    .px(px(12.0))
                    .pt(px(6.0))
                    .pb(px(2.0))
                    .text_size(px(11.0))
                    .text_color(theme.text_muted)
                    .child(text)
                    .into_any_element(),
                Entry::Item { label, icon, danger, disabled, handler } => {
                    let color = if disabled {
                        theme.text_muted.opacity(0.6)
                    } else if danger {
                        danger_color
                    } else {
                        theme.text_secondary
                    };
                    let on_dismiss = self.on_dismiss.clone();
                    div()
                        .id(("menu-item", index))
                        .h(px(32.0))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .rounded(px(8.0))
                        .text_size(px(13.0))
                        .text_color(color)
                        .when(!disabled, |el| {
                            el.cursor_pointer()
                                .hover(|s| s.bg(theme.button_hover_bg))
                                .active(|s| s.bg(theme.button_pressed_bg))
                                .on_click(move |_, window, cx| {
                                    cx.stop_propagation();
                                    on_dismiss(window, cx);
                                    handler(window, cx);
                                })
                        })
                        .child(
                            div().size(px(16.0)).flex_none().flex().items_center().justify_center().children(
                                icon.map(|icon| {
                                    svg().path(icon.path()).size(px(14.0)).text_color(color.opacity(0.85))
                                }),
                            ),
                        )
                        .child(
                            div().flex_1().min_w_0().truncate().font_weight(FontWeight::MEDIUM).child(label),
                        )
                        .into_any_element()
                }
            })
            .collect();

        deferred(
            anchored().position(self.position).snap_to_window_with_margin(px(8.0)).child(
                div()
                    .id(self.id)
                    .occlude()
                    .w(px(self.width))
                    .p(px(4.0))
                    .flex()
                    .flex_col()
                    .rounded(px(12.0))
                    .bg(theme.surface_panel_bg.opacity(1.0))
                    .border_1()
                    .border_color(theme.surface_panel_border)
                    .shadow(vec![gpui::BoxShadow {
                        color: gpui::hsla(0.0, 0.0, 0.0, if theme.dark { 0.45 } else { 0.16 }),
                        offset: gpui::point(px(0.0), px(6.0)),
                        blur_radius: px(18.0),
                        spread_radius: px(0.0),
                    }])
                    .font_family(theme.font_family.clone())
                    .on_mouse_down_out(move |_, window, cx| on_dismiss(window, cx))
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .children(entries),
            ),
        )
        .with_priority(1)
    }
}
