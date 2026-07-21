import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { basename, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(scriptDir, "..");
const outputPath = resolve(repositoryRoot, "third-party-licenses/pnpm-production-dependencies.txt");

const licenseFilePattern = /^(licen[cs]e|copying|copyright|notice|unlicense)(?:[-._].*)?$/i;
const approvedLicenses = new Set([
  "0BSD",
  "Apache-2.0",
  "Apache-2.0 OR MIT",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "BlueOak-1.0.0",
  "CC-BY-4.0",
  "ISC",
  "MIT",
  "MIT OR Apache-2.0",
  "OFL-1.1",
  "Python-2.0",
]);

function readLicenseText(path) {
  const buffer = readFileSync(path);
  if (buffer.includes(0)) {
    return null;
  }

  return buffer.toString("utf8").replaceAll("\r\n", "\n").trim();
}

function findLicenseFiles(packagePath) {
  if (!existsSync(packagePath)) {
    return [];
  }

  return readdirSync(packagePath)
    .filter((name) => licenseFilePattern.test(name))
    .map((name) => resolve(packagePath, name))
    .filter((path) => {
      try {
        return statSync(path).isFile();
      } catch {
        return false;
      }
    })
    .sort();
}

function packageLabel(entry) {
  return `${entry.name}@${entry.versions.join(", ")}`;
}

function renderInventory(entries) {
  return entries
    .map((entry) => {
      const details = [packageLabel(entry), entry.license];
      if (entry.homepage) {
        details.push(entry.homepage);
      }
      return details.join(" | ");
    })
    .join("\n");
}

function renderLicenseTexts(texts) {
  return [...texts.values()]
    .sort((left, right) => [...left.packages][0].localeCompare([...right.packages][0]))
    .map((item) => {
      const packages = [...item.packages].sort().join(", ");
      const sourceFiles = [...item.sourceFiles].sort().join(", ");
      return [
        "=".repeat(80),
        `Used by: ${packages}`,
        `Source files: ${sourceFiles}`,
        "-".repeat(80),
        item.text,
      ].join("\n");
    })
    .join("\n\n");
}

const rawInventory = execFileSync("pnpm", ["licenses", "list", "--prod", "--json"], {
  cwd: repositoryRoot,
  encoding: "utf8",
  maxBuffer: 64 * 1024 * 1024,
});
const groupedInventory = JSON.parse(rawInventory);
const unapprovedLicenses = Object.keys(groupedInventory)
  .filter((license) => !approvedLicenses.has(license))
  .sort();
if (unapprovedLicenses.length > 0) {
  throw new Error(`Unreviewed pnpm licenses found: ${unapprovedLicenses.join(", ")}`);
}

const entries = Object.values(groupedInventory)
  .flat()
  .sort((left, right) => packageLabel(left).localeCompare(packageLabel(right)));

const texts = new Map();
const entriesWithoutText = [];

for (const entry of entries) {
  const label = packageLabel(entry);
  let foundText = false;

  for (const packagePath of entry.paths) {
    for (const licensePath of findLicenseFiles(packagePath)) {
      const text = readLicenseText(licensePath);
      if (!text) {
        continue;
      }

      foundText = true;
      const digest = createHash("sha256").update(text).digest("hex");
      const item = texts.get(digest) ?? {
        text,
        packages: new Set(),
        sourceFiles: new Set(),
      };
      item.packages.add(label);
      item.sourceFiles.add(basename(licensePath));
      texts.set(digest, item);
    }
  }

  if (!foundText) {
    entriesWithoutText.push(label);
  }
}

const report = [
  "LogFilter third-party license report: pnpm production dependencies",
  "",
  "Generated from the installed dependency graph locked by pnpm-lock.yaml.",
  "Local filesystem paths are intentionally omitted.",
  "",
  `Packages: ${entries.length}`,
  `Distinct license texts: ${texts.size}`,
  `Packages without a bundled license text: ${entriesWithoutText.length}`,
  "",
  "DEPENDENCY INVENTORY",
  "=".repeat(80),
  "package@version | declared license | homepage",
  renderInventory(entries),
  "",
  "PACKAGES WITHOUT A LOCAL LICENSE TEXT",
  "=".repeat(80),
  entriesWithoutText.length > 0 ? entriesWithoutText.join("\n") : "None",
  "",
  "LICENSE AND NOTICE TEXTS",
  renderLicenseTexts(texts),
  "",
].join("\n");

mkdirSync(dirname(outputPath), { recursive: true });
writeFileSync(outputPath, report, "utf8");
console.log(`Wrote ${outputPath}`);
