use super::*;
use std::prelude::v1::test;

fn fixture(
    cx: &mut TestAppContext,
) -> (
    WindowHandle<WorkspaceView>,
    UnboundedReceiver<Value>,
    UnboundedSender<Value>,
) {
    cx.update(initialize);
    let (requests, rx) = futures::channel::mpsc::unbounded();
    let (events, frames) = futures::channel::mpsc::unbounded();
    let window = cx.add_window(|window, cx| {
        WorkspaceView::new(
            Transport {
                requests,
                events: frames,
            },
            window,
            cx,
        )
    });
    window
        .update(cx, |view, _, _| {
            view.connected = true;
            view.connection_epoch = 3;
            view.composer_generation = 7;
        })
        .unwrap();
    (window, rx, events)
}

#[gpui::test]
fn attachment_paste_admission_requires_connected_composer_focus(cx: &mut TestAppContext) {
    let (window, mut requests, _events) = fixture(cx);
    window
        .update(cx, |view, window, cx| {
            window.focus(&view.terminal_input.read(cx).focus_handle(cx), cx);
            assert!(!view.paste_attachments(1, window, cx));
            window.focus(&view.input.read(cx).focus_handle(cx), cx);
            view.settings = true;
            assert!(!view.paste_attachments(2, window, cx));
            view.settings = false;
            assert!(view.paste_attachments(3, window, cx));
            view.connected = false;
            assert!(!view.paste_attachments(4, window, cx));
            assert!(view.error.as_deref().unwrap().contains("Reconnect"));
        })
        .unwrap();
    let mut accepted = vec![];
    while let Ok(frame) = requests.try_recv() {
        if frame["ui"] == "pasteAttachments" {
            accepted.push(frame);
        }
    }
    assert_eq!(
        accepted,
        vec![json!({"ui":"pasteAttachments","ticket":3,"generation":7,"epoch":3})]
    );
}

#[gpui::test]
fn attachment_results_require_current_composer_and_connection(cx: &mut TestAppContext) {
    let (window, _requests, _events) = fixture(cx);
    window.update(cx, |view, _, cx| {
        view.receive(json!({"uploading":2,"generation":7,"epoch":3}),cx);
        for frame in [
            json!({"uploading":10,"generation":6,"epoch":3}),
            json!({"uploading":10,"generation":7,"epoch":2}),
            json!({"uploading":10,"generation":7,"epoch":"3"}),
            json!({"uploading":10,"generation":7}),
            json!({"attachment":{"error":"Old failure"},"generation":7,"epoch":2}),
            json!({"attachment":{"name":"old.txt","path":"/old.txt"},"generation":6,"epoch":3}),
        ] {view.receive(frame,cx);}
        assert_eq!(view.uploading,2); assert!(view.attachments.is_empty()); assert!(view.error.is_none());
        view.receive(json!({"attachment":{"name":"current.txt","mime":"text/plain","path":"/current.txt"},"generation":7,"epoch":3}),cx);
        assert_eq!(view.uploading,1); assert_eq!(view.attachments.len(),1);
        view.receive(json!({"attachment":{"error":"Not admitted"},"generation":7,"epoch":3,"uploadRejected":true}),cx);
        assert_eq!(view.uploading,1); assert_eq!(view.error.as_deref(),Some("Not admitted"));
        view.connected=false;
        view.receive(json!({"attachment":{"error":"Late failure"},"generation":7,"epoch":3}),cx);
        assert_eq!(view.uploading,1); assert_eq!(view.error.as_deref(),Some("Not admitted"));
    }).unwrap();
}
