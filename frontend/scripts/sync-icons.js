// Copies the remedial inline game icons (data/remedial/inline-icons/) to
// static/icons/ so the build serves them; the repository copy is the only
// source.
import { cpSync, mkdirSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const from = join(root, '../data/remedial/inline-icons');
const to = join(root, 'static/icons');
rmSync(to, { recursive: true, force: true });
mkdirSync(to, { recursive: true });
cpSync(from, to, { recursive: true });
