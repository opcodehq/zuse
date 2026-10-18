//! Opt-in browser input conformance surface; no engine or provider connection.
use gpui::{prelude::*, *};
use std::cell::RefCell;
use zuse_viewport::input::TextInput;

thread_local! {
    static APPLICATION: RefCell<Option<ApplicationHandle>> = const { RefCell::new(None) };
}

struct InputFixture {
    first: Entity<TextInput>,
    second: Entity<TextInput>,
    submitted: usize,
    readonly: bool,
    _observations: Vec<Subscription>,
}

impl InputFixture {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first = cx.new(|cx| TextInput::new("First field", window, cx));
        let second = cx.new(|cx| TextInput::new("Second field", window, cx));
        let mut observations: Vec<_> = [&first, &second]
            .into_iter()
            .map(|field| cx.observe(field, |_, _, cx| cx.notify()))
            .collect();
        for field in [&first, &second] {
            observations.push(cx.subscribe(
                field,
                |this, _, _: &zuse_viewport::input::Submitted, cx| {
                    this.submitted += 1;
                    cx.notify();
                },
            ));
        }
        Self {
            first,
            second,
            submitted: 0,
            readonly: false,
            _observations: observations,
        }
    }
}

impl Render for InputFixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Publish read-only state so the test asserts actual editor contents,
        // including preedit text, rather than inferring success from pixels.
        if let Some(body) = web_sys::window()
            .and_then(|window| window.document())
            .and_then(|document| document.body())
        {
            let value = serde_json::json!({
                "first": self.first.read(cx).value(cx),
                "second": self.second.read(cx).value(cx),
                "submitted": self.submitted,
                "readonly": self.readonly,
                "firstError": self.first.read(cx).clipboard_error().is_some(),
                "secondError": self.second.read(cx).clipboard_error().is_some(),
            });
            let _ = body.set_attribute("data-input-fixture", &value.to_string());
        }
        div()
            .size_full()
            .bg(rgb(0x101010))
            .text_color(rgb(0xeeeeee))
            .child(div().absolute().left(px(40.)).top(px(40.)).child("First"))
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(80.))
                    .w(px(500.))
                    .h(px(32.))
                    .child(self.first.clone()),
            )
            .child(div().absolute().left(px(40.)).top(px(130.)).child("Second"))
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(170.))
                    .w(px(500.))
                    .h(px(32.))
                    .child(self.second.clone()),
            )
            .child(
                div()
                    .id("clear-focus")
                    .absolute()
                    .left(px(40.))
                    .top(px(260.))
                    .w(px(200.))
                    .h(px(32.))
                    .on_click(|_, window, _| window.blur())
                    .child("Clear focus"),
            )
            .child(
                div()
                    .id("reset-inputs")
                    .absolute()
                    .left(px(40.))
                    .top(px(320.))
                    .w(px(200.))
                    .h(px(32.))
                    .on_click(cx.listener(|this, _, window, cx| {
                        window.blur();
                        for field in [&this.first, &this.second] {
                            field.update(cx, |field, cx| {
                                field.set_readonly(false, cx);
                                field.set_value(String::new(), cx);
                            });
                        }
                        this.submitted = 0;
                        this.readonly = false;
                        cx.notify();
                    }))
                    .child("Reset"),
            )
            .child(
                div()
                    .id("paste-action")
                    .absolute()
                    .left(px(40.))
                    .top(px(380.))
                    .w(px(200.))
                    .h(px(32.))
                    .on_click(|_, window, cx| {
                        let action = cx
                            .build_action("input::Paste", None)
                            .expect("registered Paste action");
                        window.dispatch_action(action, cx);
                    })
                    .child("Paste action"),
            )
            .child(
                div()
                    .id("toggle-readonly")
                    .absolute()
                    .left(px(40.))
                    .top(px(440.))
                    .w(px(200.))
                    .h(px(32.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.readonly = !this.readonly;
                        this.first
                            .update(cx, |field, cx| field.set_readonly(this.readonly, cx));
                        cx.notify();
                    }))
                    .child("Toggle read-only"),
            )
    }
}

pub fn run() {
    gpui_platform::web_init();
    let application = gpui_platform::application()
        .with_assets(zuse_viewport::assets::Assets)
        .run_embedded(|cx: &mut App| {
            zuse_viewport::initialize(cx);
            cx.open_window(WindowOptions::default(), |window, cx| {
                cx.new(|cx| InputFixture::new(window, cx))
            })
            .expect("open browser input fixture");
            if let Some(boot) = web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.get_element_by_id("boot"))
            {
                boot.remove();
            }
            cx.activate(true);
        });
    APPLICATION.with(|owner| *owner.borrow_mut() = Some(application));
}
