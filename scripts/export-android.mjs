#!/usr/bin/env node
import { exportShared } from './export-shared.mjs';
import { rm } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
const out = new URL('../android/spectrum/build/generated/spectrum/assets/', import.meta.url);
await rm(out, { recursive: true, force: true });
console.log(`Exported shared Android library assets to ${await exportShared(fileURLToPath(new URL('spectrum/', out)))}`);
