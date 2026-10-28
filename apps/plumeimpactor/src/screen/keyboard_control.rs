//! Iced 0.14 buttons do not implement keyboard focus. Keep their layout/style/mouse
//! behavior and add the standard focus operation and Enter/Space activation.
use iced::advanced::{
    Clipboard, Layout, Shell, Widget, layout, overlay, renderer,
    widget::{
        Tree,
        operation::{Focusable, Operation},
        tree,
    },
};
use iced::widget::button::{Button as IcedButton, Status, Style};
use iced::{
    Element, Event, Length, Padding, Rectangle, Size, Theme, Vector,
    keyboard::{Key, key},
    mouse,
};

pub struct Button<'a, M: Clone> {
    inner: IcedButton<'a, M>,
    action: Option<M>,
}
pub fn button<'a, M: Clone + 'a>(content: impl Into<Element<'a, M>>) -> Button<'a, M> {
    Button {
        inner: IcedButton::new(content),
        action: None,
    }
}
impl<'a, M: Clone + 'a> Button<'a, M> {
    pub fn on_press(mut self, message: M) -> Self {
        self.inner = self.inner.on_press(message.clone());
        self.action = Some(message);
        self
    }
    pub fn on_press_maybe(mut self, message: Option<M>) -> Self {
        self.inner = self.inner.on_press_maybe(message.clone());
        self.action = message;
        self
    }
    pub fn style(mut self, style: impl Fn(&Theme, Status) -> Style + 'a) -> Self {
        self.inner = self.inner.style(style);
        self
    }
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.inner = self.inner.width(width);
        self
    }
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.inner = self.inner.padding(padding);
        self
    }
}
impl<'a, M: Clone + 'a> From<Button<'a, M>> for Element<'a, M> {
    fn from(button: Button<'a, M>) -> Self {
        focusable(button.inner, button.action)
    }
}
pub fn focusable<'a, M: Clone + 'a>(
    content: impl Into<Element<'a, M>>,
    action: Option<M>,
) -> Element<'a, M> {
    Element::new(Control {
        content: content.into(),
        action,
    })
}
struct Control<'a, M> {
    content: Element<'a, M>,
    action: Option<M>,
}
#[derive(Default)]
struct Focus {
    focused: bool,
    keyboard_pressed: bool,
}
impl Focusable for Focus {
    fn is_focused(&self) -> bool {
        self.focused
    }
    fn focus(&mut self) {
        self.focused = true;
    }
    fn unfocus(&mut self) {
        self.focused = false;
        self.keyboard_pressed = false;
    }
}
impl<M: Clone> Widget<M, Theme, iced::Renderer> for Control<'_, M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Focus>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(Focus::default())
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(self.content.as_widget())]
    }
    fn diff(&self, tree: &mut Tree) {
        tree.children[0].diff(self.content.as_widget());
    }
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }
    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.action.is_some() {
            operation.focusable(None, layout.bounds(), tree.state.downcast_mut::<Focus>());
        }
        self.content
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        let focused = tree.state.downcast_mut::<Focus>();
        if matches!(
            event,
            Event::Keyboard(iced::keyboard::Event::KeyReleased {
                key: Key::Named(key::Named::Enter | key::Named::Space),
                ..
            })
        ) {
            focused.keyboard_pressed = false;
        }
        if matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        ) {
            let next = self.action.is_some() && cursor.is_over(layout.bounds());
            if next != focused.focused {
                focused.focused = next;
                shell.request_redraw();
            }
        }
        if focused.focused
            && !shell.is_event_captured()
            && let Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: Key::Named(key::Named::Enter | key::Named::Space),
                ..
            }) = event
            && let Some(action) = &self.action
        {
            if !focused.keyboard_pressed {
                focused.keyboard_pressed = true;
                shell.publish(action.clone());
            }
            shell.capture_event();
            return;
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
        if self.action.is_some() && tree.state.downcast_ref::<Focus>().focused {
            use iced::advanced::Renderer;
            renderer.fill_quad(
                renderer::Quad {
                    bounds: layout.bounds(),
                    border: iced::Border {
                        width: 2.0,
                        color: theme.palette().primary,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                },
                iced::Color::TRANSPARENT,
            );
        }
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}
