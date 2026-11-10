//! WebView2 browser controls. Selection runs in an isolated JavaScript world;
//! pages never receive a host-message bridge.
use super::*;
use serde_json::{Value, json};
use webview2_com::{CallDevToolsProtocolMethodCompletedHandler, ClearBrowsingDataCompletedHandler};

type Completion = Box<dyn FnOnce(Result<Value, String>)>;

fn protocol(web: &ICoreWebView2, method: &str, params: Value, done: Completion) {
    let done = Rc::new(RefCell::new(Some(done)));
    let callback = done.clone();
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |status, text| {
            if let Some(done) = callback.borrow_mut().take() {
                done(
                    status.map_err(|error| error.to_string()).and_then(|()| {
                        serde_json::from_str(&text).map_err(|error| error.to_string())
                    }),
                );
            }
            Ok(())
        }));
    let result = unsafe {
        web.CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(params.to_string()),
            &handler,
        )
    };
    if let Err(error) = result {
        if let Some(done) = done.borrow_mut().take() {
            done(Err(error.to_string()));
        }
    }
}

fn isolated_eval(host: &Rc<Host>, expression: String, done: Completion) {
    let Some(web) = host.page.borrow().as_ref().map(|page| page.webview.clone()) else {
        done(Err("The browser page is still starting".into()));
        return;
    };
    let weak = Rc::downgrade(host);
    let epoch = host.control_generation.get();
    let next = web.clone();
    protocol(
        &web,
        "Page.getFrameTree",
        json!({}),
        Box::new(move |tree| {
            let frame = match tree.and_then(|tree| {
                tree["frameTree"]["frame"]["id"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or("Missing browser frame".into())
            }) {
                Ok(frame) => frame,
                Err(error) => {
                    done(Err(error));
                    return;
                }
            };
            let web = next.clone();
            protocol(
                &next,
                "Page.createIsolatedWorld",
                json!({"frameId":frame,"worldName":"zuse-controls"}),
                Box::new(move |world| {
                    let context = match world.and_then(|world| {
                        world["executionContextId"]
                            .as_i64()
                            .ok_or("Missing isolated browser context".into())
                    }) {
                        Ok(context) => context,
                        Err(error) => {
                            done(Err(error));
                            return;
                        }
                    };
                    if !weak
                        .upgrade()
                        .is_some_and(|host| host.control_generation.get() == epoch)
                    {
                        done(Err("Element selection cancelled".into()));
                        return;
                    }
                    protocol(
                        &web,
                        "Runtime.evaluate",
                        json!({"contextId":context,"expression":expression,
                "awaitPromise":true,"returnByValue":true}),
                        Box::new(move |result| {
                            done(result.and_then(|value| {
                                if value.get("exceptionDetails").is_some() {
                                    Err("The page changed while selecting an element".into())
                                } else {
                                    Ok(value["result"]["value"].clone())
                                }
                            }));
                        }),
                    );
                }),
            );
        }),
    );
}

impl NativePage {
    pub(crate) fn select_element(&self, page: String, generation: u64, script: String) {
        let tx = self.0.tx.clone();
        let script = format!("(async()=>{{ {script} }})()");
        isolated_eval(
            &self.0,
            script,
            Box::new(move |result| {
                let result = result.map(|value| value.as_str().map(str::to_owned));
                let _ = tx.try_send(NativeEvent::ElementSelected {
                    page,
                    generation,
                    result,
                });
            }),
        );
    }

    pub(crate) fn update_annotation_theme(&self, palette: &str) {
        isolated_eval(
            &self.0,
            format!("globalThis.zuseSetAnnotationTheme?.({palette})"),
            Box::new(|_| {}),
        );
    }

    pub(crate) fn cancel_selection(&self) {
        self.0
            .control_generation
            .set(self.0.control_generation.get().wrapping_add(1));
        isolated_eval(
            &self.0,
            "globalThis.zuseCancelSelection?.()".into(),
            Box::new(|_| {}),
        );
    }

    pub(crate) fn toggle_devtools(&self) -> Result<(), String> {
        let page = self.0.page.borrow();
        let page = page.as_ref().ok_or("The browser page is still starting")?;
        unsafe { page.webview.OpenDevToolsWindow() }.map_err(|error| error.to_string())
    }

    pub(crate) fn save_screenshot(&self, tx: Sender) {
        let Some(web) = self
            .0
            .page
            .borrow()
            .as_ref()
            .map(|page| page.webview.clone())
        else {
            let _ = tx.try_send(NativeEvent::ScreenshotSaved(Err(
                "The browser page is still starting".into(),
            )));
            return;
        };
        protocol(
            &web,
            "Page.captureScreenshot",
            json!({"format":"png","captureBeyondViewport":false}),
            Box::new(move |result| {
                use base64::Engine;
                let bytes = result.and_then(|value| {
                    base64::engine::general_purpose::STANDARD
                        .decode(value["data"].as_str().unwrap_or_default())
                        .map_err(|error| error.to_string())
                });
                // PNG persistence belongs off the COM/UI thread.
                std::thread::spawn(move || {
                    let result = bytes.and_then(super::super::controls::save_screenshot);
                    let _ = tx.blocking_send(NativeEvent::ScreenshotSaved(result));
                });
            }),
        );
    }

    pub(crate) fn set_appearance(&self, dark: bool) {
        if self.0.dark.get() == Some(dark) {
            return;
        }
        let result = (|| -> windows::core::Result<()> {
            let page = self.0.page.borrow();
            let page = page.as_ref().ok_or_else(windows::core::Error::empty)?;
            unsafe {
                let profile = page.webview.cast::<ICoreWebView2_13>()?.Profile()?;
                profile.SetPreferredColorScheme(if dark {
                    COREWEBVIEW2_PREFERRED_COLOR_SCHEME_DARK
                } else {
                    COREWEBVIEW2_PREFERRED_COLOR_SCHEME_LIGHT
                })
            }
        })();
        if result.is_ok() {
            self.0.dark.set(Some(dark));
        }
    }
}

impl BrowserData {
    pub(crate) fn clear_data(&self, cookies: bool, tx: Sender) {
        let profile = self
            .0
            .borrow()
            .pages
            .iter()
            .filter_map(std::rc::Weak::upgrade)
            .find_map(|host| {
                let page = host.page.borrow();
                let page = page.as_ref()?;
                unsafe {
                    page.webview
                        .cast::<ICoreWebView2_13>()
                        .ok()?
                        .Profile()
                        .ok()?
                        .cast::<ICoreWebView2Profile2>()
                        .ok()
                }
            });
        let callback = tx.clone();
        let done = ClearBrowsingDataCompletedHandler::create(Box::new(move |status| {
            let event = match status {
                Ok(()) => NativeEvent::DataCleared(if cookies {
                    "Cookies cleared"
                } else {
                    "Cache cleared"
                }),
                Err(error) => {
                    NativeEvent::ControlError(format!("Could not clear browser data: {error}"))
                }
            };
            let _ = callback.try_send(event);
            Ok(())
        }));
        let result = profile
            .ok_or_else(windows::core::Error::empty)
            .and_then(|profile| unsafe {
                profile.ClearBrowsingData(
                    if cookies {
                        COREWEBVIEW2_BROWSING_DATA_KINDS_COOKIES
                    } else {
                        COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE
                    },
                    &done,
                )
            });
        if let Err(error) = result {
            let _ = tx.try_send(NativeEvent::ControlError(format!(
                "Could not clear browser data: {error}"
            )));
        }
    }
}
