import {strict as assert} from 'node:assert';
import {requestModel,modelTools,subscriptionFetch} from './model.ts';
const events=(rows:unknown[])=>new Response(rows.map(r=>`data: ${JSON.stringify(r)}\n\n`).join(''),{headers:{'content-type':'text/event-stream'}});
const completed={type:'response.completed',response:{incomplete_details:null,usage:{input_tokens:12,output_tokens:3,input_tokens_details:{cached_tokens:4}}}};
const credential={connectionId:'fixture-connection',accessToken:'fixture-token'};
let body:any;
const input={rootId:'root',agentId:'root',model:'fixture-model',messages:[{role:'system',content:'Keep requirements'},{role:'user',content:'hello'}],tools:modelTools({}),signal:new AbortController().signal,onText:(_text:string)=>{}};
let text='';
const result=await requestModel({...input,onText:t=>text+=t},credential,async(_url,init)=>{
    assert.equal(init?.redirect,'error');body=JSON.parse(String(init?.body));
    return events([
        {type:'response.created',response:{id:'response-1',created_at:1,model:'fixture-model'}},
        {type:'response.output_item.added',output_index:0,item:{id:'message-1',type:'message'}},
        {type:'response.output_text.delta',item_id:'message-1',output_index:0,delta:'hello'},
        {type:'response.output_item.done',output_index:0,item:{id:'message-1',type:'message'}},completed,
    ]);
});
assert.equal(text,'hello');assert.equal(result.text,'hello');assert.equal(result.inputTokens,12);assert.equal(result.cachedTokens,4);assert.equal(body.store,false);assert.equal(body.stream,true);assert.ok(Array.isArray(body.input));assert.ok(!body.input.some((i:any)=>i.role==='system'));
const toolResult=await requestModel({...input,tools:modelTools({edit:{description:'Edit file',parameters:{type:'object',properties:{path:{type:'string'}},required:['path']}}})},credential,async()=>events([
    {type:'response.created',response:{id:'response-2',created_at:1,model:'fixture-model'}},
    {type:'response.output_item.added',output_index:0,item:{id:'fc-1',type:'function_call',call_id:'call-1',name:'edit',namespace:'zuse',arguments:'',status:'in_progress'}},
    {type:'response.function_call_arguments.delta',item_id:'fc-1',output_index:0,delta:'{"path":"a"}'},
    {type:'response.output_item.done',output_index:0,item:{id:'fc-1',type:'function_call',call_id:'call-1',name:'edit',namespace:'zuse',arguments:'{"path":"a"}',status:'completed'}},completed,
]));
assert.equal(toolResult.calls[0].name,'edit');assert.deepEqual(toolResult.calls[0].input,{path:'a'});
await assert.rejects(()=>requestModel(input,credential,async()=>events([{type:'response.output_text.delta',item_id:'message-1',delta:'partial'}])),/stream_incomplete|stream_interrupted/);
await assert.rejects(()=>subscriptionFetch(async()=>{throw new Error('must not fetch');})('https://api.openai.com/v1/responses',{body:JSON.stringify({store:false,stream:true,input:[],temperature:1})}),/unsupported_request_field/);
console.log('Model SDK transport: text, tool calls, usage, subscription request fields, and incomplete streams verified.');
