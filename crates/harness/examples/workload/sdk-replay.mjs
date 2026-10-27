// Real SDK serialization and streaming, with no outbound model requests.
const encoder = new TextEncoder();
globalThis.fetch = async (url, init) => {
  if (String(url) !== 'https://api.openai.com/v1/responses') {
    throw new Error('Unexpected network request in local replay');
  }
  const body = JSON.parse(init.body);
  if (body.store !== false || body.stream !== true) throw new Error('Invalid replay request');
  const request = Number(body.model.replace('local-replay-', ''));
  if (!Number.isSafeInteger(request) || request < 0) throw new Error('Invalid replay model');
  const stream = new ReadableStream({start(controller) {
    const event = value => controller.enqueue(encoder.encode(`data: ${JSON.stringify(value)}\n\n`));
    event({type:'response.created', response:{id:'replay', created_at:1, model:body.model}});
    event({type:'response.output_item.added', output_index:0, item:{id:'message', type:'message'}});
    for (let chunk = 0; chunk < 32; chunk++) {
      event({type:'response.output_text.delta', item_id:'message', output_index:0, delta:'Deterministic replay response.\n'});
    }
    event({type:'response.output_item.done', output_index:0, item:{id:'message', type:'message'}});
    if (request % 2 === 0) {
      const call = {id:'function', type:'function_call', call_id:`read-${request}`, name:'read_fixture', namespace:'zuse'};
      event({type:'response.output_item.added', output_index:1, item:{...call, arguments:'', status:'in_progress'}});
      event({type:'response.function_call_arguments.delta', item_id:'function', output_index:1, delta:'{}'});
      event({type:'response.output_item.done', output_index:1, item:{...call, arguments:'{}', status:'completed'}});
    }
    event({type:'response.completed', response:{incomplete_details:null, usage:{input_tokens:1, output_tokens:1}}});
    controller.close();
  }});
  return new Response(stream, {headers:{'content-type':'text/event-stream'}});
};
