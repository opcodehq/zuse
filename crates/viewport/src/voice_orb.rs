//! Portable GPUI painter for the shared voice orb geometry.
use gpui::{prelude::*, *};
use std::{cell::RefCell, rc::Rc, time::Duration};
use web_time::Instant;
use zuse_orb::{OrbAnimator, OrbSize, OrbState, engine::Frame};
pub(super) struct VoiceOrb {
    animation: OrbAnimator,
    pub visible: bool,
    reduced: bool,
    frame: Rc<RefCell<Frame>>,
    _tick: Task<()>,
}
impl VoiceOrb {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let tick = cx.spawn(async move |this, cx| {
            loop {
                let wait = this
                    .update(cx, |orb, cx| {
                        if orb.visible && !orb.reduced {
                            cx.notify();
                            33
                        } else {
                            1000
                        }
                    })
                    .unwrap_or(1000);
                cx.background_executor()
                    .timer(Duration::from_millis(wait))
                    .await;
                if this.upgrade().is_none() {
                    break;
                }
            }
        });
        Self {
            visible: true,
            reduced: false,
            animation: OrbAnimator::new(OrbState::Connecting, OrbSize::Hero)
                .with_transition(Duration::from_millis(240)),
            frame: Rc::new(RefCell::new(Frame::new())),
            _tick: tick,
        }
    }
    pub fn update(
        &mut self,
        state: OrbState,
        microphone: f32,
        speaker: f32,
        reduced: bool,
    ) -> bool {
        let changed =
            self.animation.set_state(state, &self.frame.borrow()) || self.reduced != reduced;
        self.reduced = reduced;
        if !reduced {
            self.animation
                .set_audio_levels(microphone, speaker, Instant::now());
        }
        changed
    }
}
impl Render for VoiceOrb {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.animation
            .tick(Instant::now(), self.visible, self.reduced);
        self.animation.draw(&mut self.frame.borrow_mut());
        let frame = self.frame.clone();
        let light = crate::palette::is_light();
        let edge = self.animation.pixels();
        div().size(px(256.)).child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let scale = 256. / edge;
                    window.paint_layer(bounds, |window| {
                        let color = |white: f32, alpha: f32| {
                            opaque_grey(if light { white } else { 1. - white }, alpha.clamp(0., 1.))
                        };
                        for line in &frame.borrow().lines {
                            let mut path = PathBuilder::stroke(px(line.w * scale));
                            path.move_to(
                                bounds.origin + point(px(line.x1 * scale), px(line.y1 * scale)),
                            );
                            path.line_to(
                                bounds.origin + point(px(line.x2 * scale), px(line.y2 * scale)),
                            );
                            if let Ok(path) = path.build() {
                                window.paint_path(path, color(line.white, line.a));
                            }
                        }
                        for dot in &frame.borrow().dots {
                            let r = dot.r.max(0.3) * scale;
                            window.paint_quad(quad(
                                Bounds::new(
                                    bounds.origin
                                        + point(px(dot.x * scale - r), px(dot.y * scale - r)),
                                    size(px(2. * r), px(2. * r)),
                                ),
                                px(r),
                                color(dot.white, dot.a),
                                px(0.),
                                transparent_black(),
                                BorderStyle::default(),
                            ));
                        }
                    });
                },
            )
            .size_full(),
        )
    }
}
