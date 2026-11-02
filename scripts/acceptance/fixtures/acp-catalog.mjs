// Public registry metadata is deterministic here; configuration and probes stay real.
import path from 'node:path';
const command={command:'python3',args:[path.resolve('crates/engine/tests/fixtures/custom-acp.py'),'acp-ui','literal ; $(not-a-shell)'],env:{ACP_TEST_MARKER:'acp-ui',PARITY_TOKEN:'fake private value'}};
export const catalog={platform:'linux-x86_64',error:null,agents:[
 {id:'catalogue-peer',name:'Catalogue Agent',description:'A deterministic local ACP peer',repository:'https://example.test/agent',icon:'fixture',distribution:{local:command}},
 {id:'codex-acp',name:'Codex',description:'Built-in provider',distribution:{}},
 {id:'manual-peer',name:'Manual Agent',description:'An installed command template',distribution:{binary:{'linux-x86_64':{cmd:'python3',args:command.args,env:command.env}}}},
 {id:'other-platform',name:'Unavailable Agent',description:'Another computer platform',distribution:{binary:{'darwin-aarch64':{cmd:'unavailable'}}}},
]};
export const icon='<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path fill="currentColor" d="M2 2h12v12H2z"/></svg>';
