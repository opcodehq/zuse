import fs from 'node:fs';
import path from 'node:path';

// Isolated history plus a fresh pinned public-rate cache; no network prices or private histories.
export function seedUsagePricing(root) {
  const source = path.join(root,'cli');
  fs.mkdirSync(path.join(source,'projects'),{recursive:true});
  const cache = path.join(root,'profile','profiles','local','usage-pricing');
  fs.mkdirSync(cache,{recursive:true});
  fs.copyFileSync('crates/usage/data/model-prices.json',path.join(cache,'model-prices.json'));
  const rows = [];
  const at = Date.now() - 1000;
  for(let i=0;i<7;i++) {
    const date = new Date(at - i * 86400000);
    rows.push({sessionId:'estimated-day-'+i,timestamp:date.toISOString(),cwd:path.join(root,'archive-project'),message:{id:'estimated-'+i,model:'claude-opus-4-6',usage:{input_tokens:100000,output_tokens:10000,cache_read_input_tokens:30000,cache_creation_input_tokens:10000,speed:i===0?'fast':'standard'}}});
  }
  for(const [id,model,cost] of [['logged','claude-opus-4-6',0.25],['free','claude-opus-4-6',0],['unpriced','unpublished-xyz',null]]) {
    rows.push({sessionId:id,timestamp:new Date(at).toISOString(),cwd:path.join(root,'archive-project'),costUSD:cost,message:{id,model,usage:{input_tokens:100,output_tokens:10}}});
  }
  fs.writeFileSync(path.join(source,'projects','history.jsonl'),rows.map(row=>JSON.stringify(row)+'\n').join(''));
  const isolated = path.join(root,'empty');fs.mkdirSync(isolated);
  return {CLAUDE_CONFIG_DIR:source,CODEX_HOME:isolated,OPENCODE_DATA_DIR:isolated,AMP_DATA_DIR:isolated,PI_AGENT_DIR:isolated,GROK_DATA_DIR:isolated,ZUSE_USER_DATA_DIR:isolated};
}
