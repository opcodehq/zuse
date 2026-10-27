import {build} from 'esbuild';
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
const root=path.dirname(fileURLToPath(import.meta.url));
process.chdir(root);
const result=await build({entryPoints:['main.ts'],outfile:'model-bridge.mjs',bundle:true,platform:'node',target:'node22',format:'esm',banner:{js:"import {createRequire as zuseCreateRequire} from 'node:module'; const require=zuseCreateRequire(import.meta.url);"},minify:true,legalComments:'eof',metafile:true,write:false});
const packages=new Map();
for(const input of Object.keys(result.metafile.inputs)){
    if(!input.includes('node_modules/'))continue;
    let directory=path.dirname(path.resolve(input));
    while(directory!==root){
        try {const pkg=JSON.parse(await fs.readFile(path.join(directory,'package.json'),'utf8'));if(pkg.name){packages.set(`${pkg.name}@${pkg.version}`,{directory,pkg});break;}}catch{}
        const parent=path.dirname(directory);if(parent===directory)throw new Error('Package provenance could not be resolved.');directory=parent;
    }
}
let licenses='Third-party code bundled into model-bridge.mjs. Application transport code is AGPL-3.0-only.\n';
for(const [key,{directory,pkg}] of [...packages].sort(([a],[b])=>a<b?-1:a>b?1:0)){
    const name=pkg.name;
    licenses+=`\n${'='.repeat(72)}\n${name} ${pkg.version} (${pkg.license ?? 'see license'})\n`;
    const files=(await fs.readdir(directory)).filter(f=>/^(license|notice|copying)/i.test(f)).sort();
    if(!files.length){
        if(name.startsWith('@ai-sdk/') && pkg.license==='Apache-2.0'){licenses+=`\nSDK license notice\n${await fs.readFile('node_modules/ai/LICENSE','utf8')}\n`;continue;}
        throw new Error(`Missing bundled package license: ${name}`);
    }
    for(const file of files)licenses+=`\n${file}\n${await fs.readFile(path.join(directory,file),'utf8')}\n`;
}
licenses+=`\n${await fs.readFile('LICENSE-APACHE-2.0','utf8')}\n`;
// Cargo tracks the embedded bundle. Identical output must not trigger another
// Rust rebuild, and license collection must succeed before publishing outputs.
for(const output of result.outputFiles)await writeIfChanged(output.path,output.contents);
await writeIfChanged(path.join(root,'THIRD_PARTY_LICENSES.txt'),Buffer.from(licenses.trimEnd()+'\n'));
console.log(`Built pinned model SDK bridge; retained ${packages.size} package licenses.`);

async function writeIfChanged(file,contents){
    let previous;
    try{previous=await fs.readFile(file);}catch(error){if(error.code!=='ENOENT')throw error;}
    if(!previous?.equals(contents))await fs.writeFile(file,contents);
}
