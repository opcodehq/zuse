// SDK translation only. Rust schedules requests, chooses credentials, executes
// tools, authorizes calls, retries, and commits every durable state transition.
import {createInterface} from "node:readline";
import {requestModel,modelTools,HarnessModelError} from "./model.ts";
const active=new Map<number,AbortController>();
const emit=(id:number,frame:Record<string,unknown>)=>process.stdout.write(JSON.stringify({id,...frame})+"\n");
const input=createInterface({input:process.stdin,crlfDelay:Infinity});
input.on("line",line=>{
    if(line.length>96*1024*1024){process.exitCode=1;input.close();return;}
    let frame:any;
    try{frame=JSON.parse(line);}catch{return;}
    if(!Number.isSafeInteger(frame.id))return;
    if(frame.cancel===true){active.get(frame.id)?.abort();return;}
    if(active.has(frame.id) || active.size>=128){emit(frame.id,{type:"error",code:"bridge_busy"});return;}
    const controller=new AbortController();active.set(frame.id,controller);
    void(async()=>{
        try {
            const result=await requestModel({...frame.input,tools:modelTools(frame.input.tools),signal:controller.signal,onText:text=>emit(frame.id,{type:"text",text})},frame.credential);
            emit(frame.id,{type:"result",result});
        }catch(error){emit(frame.id,{type:"error",code:error instanceof HarnessModelError?error.code:controller.signal.aborted?"interrupted":"bridge_failed",status:error instanceof HarnessModelError?error.status:undefined});}
        finally{active.delete(frame.id);}
    })();
});
input.on("close",()=>{for(const c of active.values())c.abort();});
