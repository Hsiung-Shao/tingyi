import { build } from 'esbuild';
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.dirname(fileURLToPath(import.meta.url));
const out = path.join(root, 'dist');
rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });

await build({
  entryPoints: {
    background: path.join(root, 'src/background/index.ts'),
    content: path.join(root, 'src/content/index.ts'),
    // YouTube 頁面世界(MAIN world):讀播放器有沒有字幕,轉告 content script
    'yt-main': path.join(root, 'src/content/ytMain.ts'),
    'popup/popup': path.join(root, 'src/popup/popup.ts'),
    'options/options': path.join(root, 'src/options/options.ts'),
  },
  outdir: out,
  bundle: true,
  // 每個入口獨立 IIFE:content script 頂層變數不會汙染頁面或撞名
  format: 'iife',
  platform: 'browser',
  target: 'chrome120',
  logLevel: 'warning',
});

const manifest = JSON.parse(readFileSync(path.join(root, 'public/manifest.json'), 'utf8'));
// 固定 key → 固定擴充 ID,bridge 的 Origin 白名單才對得上
manifest.key = JSON.parse(readFileSync(path.join(root, 'public/key.json'), 'utf8')).key;
writeFileSync(path.join(out, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');

cpSync(path.join(root, 'src/content/style.css'), path.join(out, 'content.css'));
cpSync(path.join(root, 'src/shared/ui.css'), path.join(out, 'shared/ui.css'));
cpSync(path.join(root, 'src/popup/popup.html'), path.join(out, 'popup/popup.html'));
cpSync(path.join(root, 'src/options/options.html'), path.join(out, 'options/options.html'));
console.log(`built → ${out}`);
