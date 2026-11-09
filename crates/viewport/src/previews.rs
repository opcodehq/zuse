//! Bounded RPC image reads and normalized thumbnails for the shared workspace.
use super::*;
use crate::localization::message as tr;
use crate::shell::{column, control, label};
use base64::Engine as _;
use image::ImageDecoder as _;
use std::{io::Cursor, sync::Arc};
use zuse_i18n::messages as msg;

use zuse_protocol::message_attachments::AttachmentBytes;
const MAX_PREVIEWS: usize = 8;
const MAX_PARALLEL: usize = 2;

pub(super) enum Preview {
    Loading {
        bytes: AttachmentBytes,
    },
    Decoding,
    Ready {
        image: Arc<Image>,
        width: u32,
        height: u32,
    },
    Error(String),
}

fn thumbnail(bytes: Vec<u8>) -> Result<(Arc<Image>, u32, u32), String> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| "Could not identify this image.")?;
    if !matches!(
        reader.format(),
        Some(
            image::ImageFormat::Png
                | image::ImageFormat::Jpeg
                | image::ImageFormat::WebP
                | image::ImageFormat::Gif
                | image::ImageFormat::Bmp
                | image::ImageFormat::Tiff
                | image::ImageFormat::Ico
        )
    ) {
        return Err("This image format cannot be previewed.".into());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(96 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| "Could not decode this image within preview limits.")?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut decoded = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| "Could not decode this image within preview limits.")?;
    decoded.apply_orientation(orientation);
    let image = decoded.thumbnail(1024, 768);
    let (width, height) = (image.width(), image.height());
    let mut png = Cursor::new(Vec::new());
    image
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(|_| "Could not prepare image preview.")?;
    Ok((
        Arc::new(Image::from_bytes(ImageFormat::Png, png.into_inner())),
        width,
        height,
    ))
}

impl WorkspaceView {
    pub(super) fn interrupt_previews(&mut self) {
        self.preview_pending.clear();
        self.previews
            .retain(|_, preview| !matches!(preview, Preview::Loading { .. }));
    }
    pub(super) fn reset_previews(&mut self, cx: &mut Context<Self>) {
        self.preview_generation = self.preview_generation.wrapping_add(1);
        self.preview_open = None;
        for id in std::mem::take(&mut self.preview_pending).into_keys() {
            self.cancel(id);
        }
        for preview in std::mem::take(&mut self.previews).into_values() {
            if let Preview::Ready { image, .. } = preview {
                ImageSource::Image(image).evict(None, cx);
            }
        }
    }

    pub(super) fn sync_previews(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if !self.connected {
            return;
        }
        let mut paths: Vec<String> = self
            .attachments
            .iter()
            .filter_map(|a| a["path"].as_str())
            .filter(|p| zuse_protocol::attachment_mentions::is_image_path(p))
            .map(str::to_owned)
            .collect();
        for message in self.messages.iter().rev() {
            for part in &message.parts {
                match part {
                    zuse_model::MessagePart::Image { path, .. } => paths.push(path.clone()),
                    zuse_model::MessagePart::Text { text, .. }
                        if message.role == zuse_model::MessageRole::User =>
                    {
                        paths.extend(
                            zuse_protocol::message_attachments::parse(text)
                                .attachments
                                .into_iter()
                                .filter(|a| {
                                    zuse_protocol::attachment_mentions::is_image_path(&a.path)
                                })
                                .map(|a| a.path),
                        );
                    }
                    _ => {}
                }
            }
            if paths.len() >= MAX_PREVIEWS {
                break;
            }
        }
        let mut seen = std::collections::HashSet::new();
        paths.retain(|path| seen.insert(path.clone()));
        paths.truncate(MAX_PREVIEWS);
        if refresh {
            let retired: Vec<String> = self
                .previews
                .iter()
                .filter(|(path, preview)| {
                    !paths.contains(path)
                        && !matches!(preview, Preview::Loading { .. } | Preview::Decoding)
                })
                .map(|(path, _)| path.clone())
                .collect();
            for path in retired {
                if let Some(Preview::Ready { image, .. }) = self.previews.remove(&path) {
                    ImageSource::Image(image).evict(None, cx);
                }
            }
        }
        for path in paths {
            if self.previews.len() >= MAX_PREVIEWS {
                break;
            }
            if !self.previews.contains_key(&path) && self.preview_active() < MAX_PARALLEL {
                self.load_preview(path, cx);
            }
        }
    }

    fn preview_active(&self) -> usize {
        self.previews
            .values()
            .filter(|p| matches!(p, Preview::Loading { .. } | Preview::Decoding))
            .count()
    }

    pub(super) fn load_preview(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.require_connection(cx) {
            return;
        }
        if self.device_id.is_none() || self.preview_active() >= MAX_PARALLEL {
            return;
        }
        if matches!(
            self.previews.get(&path),
            Some(Preview::Loading { .. } | Preview::Decoding | Preview::Ready { .. })
        ) {
            return;
        }
        if self.previews.len() >= MAX_PREVIEWS && !self.previews.contains_key(&path) {
            let evict = self
                .previews
                .iter()
                .find(|(_, p)| !matches!(p, Preview::Loading { .. } | Preview::Decoding))
                .map(|(k, _)| k.clone());
            let Some(evict) = evict else {
                return;
            };
            if let Some(Preview::Ready { image, .. }) = self.previews.remove(&evict) {
                ImageSource::Image(image).evict(None, cx);
            }
        }
        self.previews.insert(
            path.clone(),
            Preview::Loading {
                bytes: AttachmentBytes::default(),
            },
        );
        self.read_preview_chunk(path, 0);
        cx.notify();
    }

    fn read_preview_chunk(&mut self, path: String, offset: u64) {
        let device = self
            .selected
            .as_ref()
            .map(|chat| chat["deviceId"].clone())
            .unwrap_or_else(|| json!(self.device_id));
        let id = self.call(
            methods::READ_ATTACHMENT_CHUNK,
            json!({"path":path,"offset":offset,"targetDeviceId":device}),
        );
        self.preview_pending.insert(id, path);
    }

    pub(super) fn receive_preview(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(path) = self.preview_pending.get(&id).cloned() else {
            return false;
        };
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        self.preview_pending.remove(&id);
        self.pending.remove(&id);
        let result = (|| {
            if let Some(error) = frame["err"].as_str() {
                return Err(error.to_owned());
            }
            let value = &frame["ok"];
            let data = value["data"]
                .as_str()
                .ok_or("Image response is missing its bytes.")?;
            // Bound the frame before allocating the decoded binary.
            if data.len() > 128 * 1024 {
                return Err("Image response exceeded the chunk limit.".into());
            }
            let chunk = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| "Image response contained invalid bytes.")?;
            let next = value["nextOffset"]
                .as_u64()
                .ok_or("Image response is missing its cursor.")?;
            let done = value["done"]
                .as_bool()
                .ok_or("Image response is missing its completion state.")?;
            let Some(Preview::Loading { bytes }) = self.previews.get_mut(&path) else {
                return Err("Image read was retired.".into());
            };
            bytes.append(&chunk, next, done)?;
            Ok(done)
        })();
        match result {
            Err(error) => {
                self.previews.insert(path, Preview::Error(error));
                self.sync_previews(false, cx);
            }
            Ok(false) => {
                let offset = match self.previews.get(&path) {
                    Some(Preview::Loading { bytes }) => bytes.offset(),
                    _ => 0,
                };
                self.read_preview_chunk(path, offset);
            }
            Ok(true) => {
                let Some(Preview::Loading { bytes, .. }) =
                    self.previews.insert(path.clone(), Preview::Decoding)
                else {
                    return true;
                };
                let generation = self.preview_generation;
                let decode = cx
                    .background_executor()
                    .spawn(async move { thumbnail(bytes.into_bytes()) });
                cx.spawn(async move |this, cx| {
                    let result = decode.await;
                    let _ = this.update(cx, |this, cx| {
                        if generation != this.preview_generation {
                            return;
                        }
                        this.previews.insert(
                            path,
                            match result {
                                Ok((image, width, height)) => Preview::Ready {
                                    image,
                                    width,
                                    height,
                                },
                                Err(error) => Preview::Error(error),
                            },
                        );
                        this.sync_previews(false, cx);
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        cx.notify();
        true
    }

    pub(super) fn image_preview(
        &self,
        path: &str,
        name: &str,
        max_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut card = column()
            .max_w(px(max_width))
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x292929))
            .overflow_hidden();
        match self.previews.get(path) {
            Some(Preview::Ready {
                image,
                width,
                height,
            }) => {
                let w = (*width as f32).min(max_width);
                let h = w * *height as f32 / (*width).max(1) as f32;
                let open_path = path.to_owned();
                card = card.child(
                    div()
                        .id(SharedString::from(format!("preview-{path}")))
                        .role(Role::Button)
                        .aria_label(tr(cx, msg::native::enlarge_image))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.preview_open = Some(open_path.clone());
                            cx.notify();
                        }))
                        .child(
                            img(image.clone())
                                .id(SharedString::from(format!("image-texture-{path}")))
                                .with_loading(|| {
                                    label("")
                                        .child(crate::localization::element(
                                            msg::native::preparing_preview,
                                        ))
                                        .p_4()
                                        .into_any_element()
                                })
                                .with_fallback(|| {
                                    label("")
                                        .child(crate::localization::element(
                                            msg::native::could_not_render_this_preview,
                                        ))
                                        .p_4()
                                        .into_any_element()
                                })
                                .w(px(w))
                                .h(px(h))
                                .object_fit(ObjectFit::Contain),
                        ),
                );
            }
            Some(Preview::Loading { .. } | Preview::Decoding) => {
                card = card.child(label(tr(cx, msg::chat::image_gallery_loading)).p_4());
            }
            state => {
                let path = path.to_owned();
                card = card.child(
                    column()
                        .p_3()
                        .gap_2()
                        .when_some(
                            match state {
                                Some(Preview::Error(error)) => Some(error.clone()),
                                _ => None,
                            },
                            |d, error| d.child(label(error)),
                        )
                        .child(
                            control(
                                SharedString::from(format!("load-image-{path}")),
                                tr(cx, msg::native::load_preview),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.load_preview(path.clone(), cx),
                            )),
                        ),
                );
            }
        }
        card.when(!name.is_empty(), |card| {
            card.child(label(name.to_owned()).px_3().py_2().text_size(px(11.)))
        })
        .into_any_element()
    }

    pub(super) fn preview_overlay(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(path) = &self.preview_open else {
            return div().into_any_element();
        };
        let Some(Preview::Ready {
            image,
            width,
            height,
        }) = self.previews.get(path)
        else {
            return div().into_any_element();
        };
        let bounds = window.viewport_size();
        let scale = ((f32::from(bounds.width) - 64.).max(1.) / *width as f32)
            .min((f32::from(bounds.height) - 128.).max(1.) / *height as f32)
            .min(1.);
        div()
            .id("image-preview-dialog")
            .role(Role::Dialog)
            .aria_label(tr(cx, msg::native::image_preview))
            .absolute()
            .inset_0()
            .bg(rgba(0x000000cc))
            .flex()
            .items_center()
            .justify_center()
            .child(
                column()
                    .rounded_xl()
                    .overflow_hidden()
                    .bg(rgb(0x181818))
                    .border_1()
                    .border_color(rgb(0x383838))
                    .child(
                        crate::shell::row()
                            .h(px(40.))
                            .px_3()
                            .gap_3()
                            .child(
                                label(
                                    zuse_protocol::attachment_mentions::attachment_display_name(
                                        path.rsplit('/').next().unwrap_or("Image"),
                                    )
                                    .to_owned(),
                                )
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis(),
                            )
                            .child(
                                control("close-image-preview", tr(cx, msg::common::close))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.preview_open = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .child(
                        img(image.clone())
                            .w(px(*width as f32 * scale))
                            .h(px(*height as f32 * scale))
                            .object_fit(ObjectFit::Contain),
                    ),
            )
            .into_any_element()
    }
}
