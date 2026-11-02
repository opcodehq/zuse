// Provider limits are protocol data; no provider, authentication or payment service runs.
export const sidebarAccounts = [
  {id:'claude-avery',harness:'claude-code',email:'avery@example.test',planLabel:'Claude Pro',active:true,
   usageWindows:[{label:'Session',usedFraction:.55,resetsAt:new Date(Date.now()+2*3600000).toISOString()},{label:'Weekly',usedFraction:.2,resetsAt:new Date(Date.now()+3*86400000).toISOString()}]},
  {id:'codex-avery',harness:'codex',email:'avery@example.test',planLabel:'Codex Team',active:true,
   usageWindows:[{label:'Session',usedFraction:.88,resetsAt:new Date(Date.now()+3600000).toISOString()}]},
];
