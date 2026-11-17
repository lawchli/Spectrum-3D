#!/usr/bin/env node
// Native consumers share the web specification; generated assets are never a second source.
import { mkdir, readdir, copyFile, writeFile, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { LOOK } from '../web/src/look.js';
import { LOOK as DADASPEC_LOOK } from '../web/styles/dadaspec/look.js';
import { STRINGS } from '../web/src/i18n.js';
import { paletteBytes } from '../web/src/gl.js';

const nativeStrings = {
  zh: {
    openingFile: '正在打开音频文件…', outputUnavailable: '无法播放声音：{msg}',
    stop: '停止', systemAudio: '系统内录',
    captureNotificationTitle: 'Spectrum 3D 正在采集音频',
    captureNotificationText: '正在采集系统音频，点击停止可结束采集',
    captureChannelName: '音频采集', antiAliasing: '抗锯齿', msaaOff: '关',
    'palette.dadaspec': 'Dadaspec',
    restartRequired: '重启应用后生效', microphonePermissionDenied: '麦克风权限被拒绝',
    'tip.renderScale': '按比例缩小 3D 频谱的渲染分辨率后再放大到画框；文字与界面始终保持原分辨率。选“自动”会在掉帧时先试 85%、70%，帧率稳住后再升回。',
    'tip.msaa': '4× 多重采样让网格和频谱线条更平滑；高分屏或集显上关闭可明显减少 GPU 像素开销，立即生效。',
    sourceOff: '关闭', microphone: '麦克风', deviceDefault: '默认输入设备',
    inputUnavailable: '输入设备不可用', freeze: '冻结', language: '语言',
    openingInput: '正在打开输入设备…', inputOpenTimeout: '输入设备打开超时（10 秒）', refreshDevices: '刷新设备',
    'mon.callback': '回调帧', 'mon.process': '进程', 'mon.lostInput': '输入丢帧', 'mon.lostRows': '丢行',
    // The web viewHint names double-click, WASD and the gamepad, which the desktop app has not.
    'desktop.viewHint': '左键拖动旋转，右键或 Shift 拖动平移，滚轮缩放。键盘：1 2 3 切换视角，R 回到当前视角，空格冻结，C 清零最大值，H 收起界面，M 开关监控。',
  },
  en: {
    openingFile: 'Opening audio file…', outputUnavailable: 'Audio output unavailable: {msg}',
    stop: 'Stop', systemAudio: 'System audio',
    captureNotificationTitle: 'Spectrum 3D is capturing audio',
    captureNotificationText: 'Capturing system audio. Stop to end capture.',
    captureChannelName: 'Audio capture', antiAliasing: 'Anti-aliasing', msaaOff: 'Off',
    'palette.dadaspec': 'Dadaspec',
    restartRequired: 'Applies after restart', microphonePermissionDenied: 'Microphone permission denied',
    'tip.renderScale': 'Renders the 3D spectrum at a fraction of the frame size and scales it up, while text and UI stay at full resolution. Auto steps down to 85% or 70% when GPU-bound and steps back up once frames stay fast.',
    'tip.msaa': '4× multisampling smooths mesh and spectrum edges. Turning it off cuts GPU pixel load on high-DPI or integrated graphics, taking effect immediately.',
    sourceOff: 'Off', microphone: 'Microphone', deviceDefault: 'Default input device',
    inputUnavailable: 'Input device unavailable', freeze: 'Freeze', language: 'Language',
    openingInput: 'Opening input device…', inputOpenTimeout: 'Input device did not open within 10 seconds', refreshDevices: 'Refresh devices',
    'mon.callback': 'callback', 'mon.process': 'proc', 'mon.lostInput': 'input drops', 'mon.lostRows': 'row drops',
    'desktop.viewHint': 'Left drag orbits, right or Shift drag pans, scroll zooms. Keys: 1 2 3 pick views, R returns to the current view, Space freezes, C clears max hold, H hides the interface, M toggles the monitor.',
  },
};

export async function exportShared(destination) {
  const path = resolve(destination);
  // Windows resolves to backslashes; compare on forward slashes.
  const posix = path.replaceAll('\\', '/').replace(/\/+$/, '');
  if (!posix.endsWith('/toolchains/spectrum-assets') && !posix.includes('/build/generated/spectrum/assets')) {
    throw Error('Refusing to replace a directory outside generated native assets');
  }
  const out = pathToFileURL(path + '/');
  // Only the explicitly selected generated-assets directory is replaced.
  await rm(out, { recursive: true, force: true });
  await mkdir(new URL('shaders/', out), { recursive: true });
  const shaderRoot = new URL('../web/shaders/', import.meta.url);
  for (const name of await readdir(shaderRoot)) {
    await copyFile(new URL(name, shaderRoot), new URL(`shaders/${name}`, out));
  }
  await writeFile(new URL('look.json', out), JSON.stringify(LOOK));
  await writeFile(new URL('look-dadaspec.json', out), JSON.stringify(DADASPEC_LOOK));
  const strings = Object.fromEntries(Object.entries(STRINGS).map(([lang, values]) => [lang, { ...values, ...nativeStrings[lang] }]));
  await writeFile(new URL('strings.json', out), JSON.stringify(strings));
  for (const [name, stops] of Object.entries(LOOK.palettes)) {
    await writeFile(new URL(`palette-${name}.rgba`, out), paletteBytes(stops));
  }
  for (const [name, stops] of Object.entries(DADASPEC_LOOK.palettes)) {
    await writeFile(new URL(`palette-${name}.rgba`, out), paletteBytes(stops));
  }
  return fileURLToPath(out);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const output = process.argv[2] || fileURLToPath(new URL('../toolchains/spectrum-assets/', import.meta.url));
  console.log(`Exported shared native assets to ${await exportShared(output)}`);
}
