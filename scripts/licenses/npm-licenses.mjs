#!/usr/bin/env node
// Writes the license texts of the npm packages bundled into a web interface
// (Keyglass Studio's): every production package in its package-lock.json,
// plus the build tools named with --bundled whose code the build emits into
// the bundle, with every license, copying and notice file each package ships.
// One plain text file.
//
// Fails when a package is not installed, ships no license file, or declares
// a license that deny.toml's [licenses] allow list does not accept (an OR
// expression needs one accepted alternative, an AND expression all of them).
//
// Usage:
//   npm-licenses.mjs --package-dir <dir> [--bundled <package>]... -o <file>
//
// <dir> holds package.json, package-lock.json (lockfileVersion 2 or 3) and
// node_modules (`npm ci`). Needs Node.js 18 or newer.

import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function die(message) {
  console.error(`npm-licenses: ${message}`);
  process.exit(1);
}

const args = process.argv.slice(2);
let packageDir = "";
let out = "";
const bundled = [];
for (let i = 0; i < args.length; i += 2) {
  const [flag, value] = [args[i], args[i + 1]];
  if (value === undefined) die(`${flag} needs a value`);
  if (flag === "--package-dir") packageDir = value;
  else if (flag === "--bundled") bundled.push(value);
  else if (flag === "-o") out = value;
  else die(`unknown argument: ${flag}`);
}
if (!packageDir) die("--package-dir is required");
if (!out) die("-o is required");

// The allow list of deny.toml's [licenses] table.
function allowedLicenses() {
  const deny = readFileSync(join(repo, "deny.toml"), "utf8");
  const table = deny.split(/^\[licenses\]\s*$/m)[1];
  const list = table?.match(/^allow\s*=\s*\[([^\]]*)\]/m);
  if (!list) die("deny.toml has no [licenses] allow list");
  return new Set([...list[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]));
}
const allowed = allowedLicenses();

// Whether an SPDX license expression is acceptable under the allow list.
function accepted(expression) {
  const tokens = expression.match(/\(|\)|[^\s()]+/g) ?? [];
  let pos = 0;
  const peek = () => tokens[pos];
  function primary() {
    const token = tokens[pos++];
    if (token === "(") {
      const value = or();
      if (tokens[pos++] !== ")") throw new Error("unbalanced parentheses");
      return value;
    }
    if (token === undefined || token === ")" || /^(AND|OR|WITH)$/.test(token)) {
      throw new Error(`unexpected ${token ?? "end"}`);
    }
    if (peek() === "WITH") {
      pos++;
      const exception = tokens[pos++];
      if (!exception) throw new Error("WITH needs an exception");
      return allowed.has(`${token} WITH ${exception}`);
    }
    return allowed.has(token);
  }
  function and() {
    let value = primary();
    while (peek() === "AND") {
      pos++;
      value = primary() && value;
    }
    return value;
  }
  function or() {
    let value = and();
    while (peek() === "OR") {
      pos++;
      value = and() || value;
    }
    return value;
  }
  const value = or();
  if (pos !== tokens.length) throw new Error(`unexpected ${tokens[pos]}`);
  return value;
}

function declaredLicense(manifest) {
  if (typeof manifest.license === "string") return manifest.license;
  if (manifest.license?.type) return manifest.license.type;
  if (Array.isArray(manifest.licenses)) {
    return manifest.licenses.map((l) => (typeof l === "string" ? l : l.type)).join(" OR ");
  }
  return "";
}

const licenseFile = /^(licen[cs]e|copying|notice)([-._][\w.-]*)?$/i;
const codeFile = /\.(c?js|mjs|ts|json|map)$/i;

const dir = resolve(packageDir);
const lockPath = join(dir, "package-lock.json");
if (!existsSync(lockPath)) die(`no ${lockPath}`);
const lock = JSON.parse(readFileSync(lockPath, "utf8"));
if (!lock.packages) die(`${lockPath} has lockfileVersion ${lock.lockfileVersion}; 2 or newer is required`);

const paths = Object.entries(lock.packages)
  .filter(([path, entry]) => path.startsWith("node_modules/") && !entry.dev && !entry.link)
  .map(([path]) => path);
for (const name of bundled) {
  const path = `node_modules/${name}`;
  if (!lock.packages[path]) die(`--bundled ${name} is not in ${lockPath}`);
  if (!paths.includes(path)) paths.push(path);
}

const problems = [];
const packages = [];
for (const path of paths) {
  const entry = lock.packages[path];
  const root = join(dir, path);
  const manifestPath = join(root, "package.json");
  if (!existsSync(manifestPath)) {
    if (entry.optional) continue; // an optional package not installed on this platform
    problems.push(`${path} is not installed (run npm ci in ${packageDir})`);
    continue;
  }
  const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
  const name = manifest.name;
  const license = declaredLicense(manifest);
  try {
    if (!license) problems.push(`${name} ${manifest.version} declares no license`);
    else if (!accepted(license)) problems.push(`${name} ${manifest.version}: ${license} is not allowed by deny.toml`);
  } catch (error) {
    problems.push(`${name} ${manifest.version}: cannot read license expression "${license}": ${error.message}`);
  }
  const files = readdirSync(root, { withFileTypes: true })
    .filter((f) => f.isFile() && licenseFile.test(f.name) && !codeFile.test(f.name))
    .map((f) => f.name)
    .sort();
  if (files.length === 0) problems.push(`${name} ${manifest.version} ships no license file`);
  packages.push({
    name,
    version: manifest.version,
    license,
    bundledTool: bundled.includes(name),
    texts: files.map((f) => ({ file: f, text: readFileSync(join(root, f), "utf8") })),
  });
}
if (problems.length > 0) die(`\n  ${problems.join("\n  ")}`);
if (packages.length === 0) die(`${lockPath} lists no production packages`);
packages.sort((a, b) => (a.name === b.name ? a.version.localeCompare(b.version) : a.name < b.name ? -1 : 1));

const rule = (c) => `${c.repeat(78)}\n`;
let text = `npm packages
============

The licenses of the npm packages bundled into this program's interface,
generated from its package-lock.json by npm-licenses.mjs
(scripts/licenses/npm-licenses.mjs in the Keyglass source). Each package's
license, copying and notice files are reproduced as the package ships them.

Packages:
`;
for (const p of packages) {
  const note = p.bundledTool ? ", build tool; code it generates is in the bundle" : "";
  text += `  ${p.name} ${p.version} (${p.license}${note})\n`;
}
text += "\n";
for (const p of packages) {
  text += rule("=");
  text += `${p.name} ${p.version} (${p.license})\n`;
  for (const t of p.texts) {
    text += rule("-");
    text += `${t.file}:\n\n`;
    text += t.text.endsWith("\n") ? t.text : `${t.text}\n`;
  }
  text += "\n";
}

mkdirSync(dirname(resolve(out)), { recursive: true });
writeFileSync(out, text);
console.log(`npm-licenses: wrote ${out} (${packages.length} packages)`);
