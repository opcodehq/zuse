#![cfg(unix)]
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use zuse_harness::{
    bridge::{Credential, ModelBridge},
    *,
};
struct Budget;
#[async_trait::async_trait]
impl RequestBudget for Budget {
    async fn reserve(&self) -> Result<(), Error> {
        Ok(())
    }
}
fn input(
    model: &str,
    cancel: CancellationToken,
    text: Arc<dyn Fn(String) + Send + Sync>,
) -> ModelInput {
    ModelInput {
        root_id: "root".into(),
        agent_id: model.into(),
        model: model.into(),
        reasoning: None,
        messages: vec![Message::text(Role::User, "test")],
        tools: Tools::new(),
        cancel,
        text,
        budget: Arc::new(Budget),
    }
}
fn credential() -> Credential {
    Credential {
        connection_id: "fixture".into(),
        provider: "chatgpt".into(),
        access_token: "fixture-token".into(),
    }
}
#[tokio::test]
async fn sdk_process_multiplexes_requests_and_cancels_one_without_affecting_others() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let preload = directory.path().join("fetch.mjs");
    std::fs::write(&preload,r#"
const encoder=new TextEncoder();
globalThis.fetch=async(url,init)=>{
 if(String(url)!=='https://api.openai.com/v1/responses')throw new Error('unexpected fixture request');
 const body=JSON.parse(init.body);if(body.store!==false || body.stream!==true)throw new Error('invalid request');
 const name=body.model;
 const stream=new ReadableStream({start(controller){
  const event=value=>controller.enqueue(encoder.encode(`data: ${JSON.stringify(value)}\n\n`));
  event({type:'response.created',response:{id:`response-${name}`,created_at:1,model:name}});
  event({type:'response.output_item.added',output_index:0,item:{type:'message',id:`message-${name}`}});
  event({type:'response.output_text.delta',item_id:`message-${name}`,delta:name});
  if(name==='slow'){init.signal.addEventListener('abort',()=>controller.error(new DOMException('Interrupted','AbortError')),{once:true});return;}
  event({type:'response.output_item.done',output_index:0,item:{type:'message',id:`message-${name}`}});
  event({type:'response.completed',response:{usage:{input_tokens:3,output_tokens:1},incomplete_details:null}});controller.close();
 }});
 return new Response(stream,{headers:{'content-type':'text/event-stream'}});
};
"#).unwrap();
    let launcher = directory.path().join("node-fixture");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nexec node --import '{}' \"$@\"\n",
            preload.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
    let bridge = Arc::new(ModelBridge::new(directory.path().join("bridge")).with_node(launcher));
    let both = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        futures::future::join(
            bridge.request(
                input("first", CancellationToken::new(), Arc::new(|_| {})),
                credential(),
            ),
            bridge.request(
                input("second", CancellationToken::new(), Arc::new(|_| {})),
                credential(),
            ),
        ),
    )
    .await
    .unwrap();
    assert_eq!(both.0.unwrap().text, "first");
    assert_eq!(both.1.unwrap().text, "second");
    let started = Arc::new(tokio::sync::Notify::new());
    let signal = started.clone();
    let cancel = CancellationToken::new();
    let pending = {
        let bridge = bridge.clone();
        let token = cancel.clone();
        tokio::spawn(async move {
            bridge
                .request(
                    input("slow", token, Arc::new(move |_| signal.notify_one())),
                    credential(),
                )
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    cancel.cancel();
    assert!(matches!(pending.await.unwrap(), Err(Error::Interrupted)));
    assert_eq!(
        bridge
            .request(
                input("after", CancellationToken::new(), Arc::new(|_| {})),
                credential()
            )
            .await
            .unwrap()
            .text,
        "after"
    );
    bridge.close().await;
}
