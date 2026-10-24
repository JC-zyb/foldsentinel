import { readFileSync } from 'node:fs';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const read = path => readFileSync(resolve(root, path), 'utf8');
const json = path => JSON.parse(read(path));
const version = json('package.json').version;
assert.match(version, /^\d+\.\d+\.\d+$/);
for (const file of ['Cargo.toml', 'src-tauri/Cargo.toml']) {
  assert.equal(read(file).match(/^version\s*=\s*"([^"]+)"/m)?.[1], version, file);
}
assert.equal(json('src-tauri/tauri.conf.json').version, version, 'Tauri config');
const lock = json('package-lock.json');
assert.equal(lock.version, version, 'npm lock root');
assert.equal(lock.packages[''].version, version, 'npm lock package');
for (const name of ['foldsentinel', 'foldsentinel-desktop']) {
  const block = read('Cargo.lock').split('[[package]]').find(block => block.includes(`\nname = "${name}"\n`));
  assert.equal(block?.match(/\nversion = "([^"]+)"/)?.[1], version, `Cargo lock ${name}`);
}
assert.ok(read('ui/i18n.ts').includes(`v${version}`), 'UI version');
console.log(`Version consistency OK: ${version}`);
