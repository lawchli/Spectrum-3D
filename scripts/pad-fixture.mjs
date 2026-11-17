import {writeFile} from 'node:fs/promises';
import {options,withBrowser,openLab} from './browser-session.mjs';
const opts=options();
await withBrowser(opts,async a=>{
 await openLab(a,opts.url);
 const samples=await a.eval(`(async()=>{const {startTestPad}=await import('./src/audio.js');const ctx=new OfflineAudioContext(1,48000,48000);const stop=startTestPad(ctx,ctx.destination);try{const buffer=await ctx.startRendering();return Array.from(buffer.getChannelData(0));}finally{stop();}})()`);
 await writeFile('toolchains/test-pad.f32',Buffer.from(Float32Array.from(samples).buffer));
});
