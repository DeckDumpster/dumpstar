// Copies the remedial aspect icons (data/remedial/aspects/) to static/aspects/
// so the build serves them; the repository copy is the only source.
import { cpSync, mkdirSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const from = join(root, '../data/remedial/aspects');
const to = join(root, 'static/aspects');
rmSync(to, { recursive: true, force: true });
mkdirSync(to, { recursive: true });
cpSync(from, to, { recursive: true });
