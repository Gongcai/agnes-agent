import { existsSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const projectDir = resolve(scriptDir, "..");
const pdfDir = resolve(projectDir, "pdf-parser");
const parserEntry = resolve(projectDir, "document-parser", "document_parserd.py");

function run(command, args, cwd = projectDir) {
  const result = spawnSync(command, args, { cwd, stdio: "inherit" });
  if (result.status !== 0) {
    throw result.error || new Error(`${command} exited with status ${result.status}`);
  }
}

function output(command, args) {
  const result = spawnSync(command, args, { cwd: projectDir, encoding: "utf8" });
  if (result.status !== 0) throw result.error || new Error(result.stderr);
  return result.stdout;
}

const target =
  process.env.TAURI_ENV_TARGET_TRIPLE ||
  output("rustc", ["-vV"]).match(/^host:\s*(\S+)$/m)?.[1];
if (!target) throw new Error("Unable to determine the Rust host target triple");

const modelsDir = resolve(pdfDir, ".models");
if (!existsSync(resolve(modelsDir, "agnes-models.json"))) {
  run("uv", ["run", "python", "download_models.py", "--output-dir", modelsDir], pdfDir);
}

const workDir = resolve(pdfDir, ".sidecar-build", target);
const distDir = resolve(pdfDir, ".sidecar-dist", target);
const packageDir = resolve(pdfDir, ".package-dist", target);
mkdirSync(packageDir, { recursive: true });
run(
  "uv",
  [
    "run",
    "python",
    "-m",
    "PyInstaller",
    "--clean",
    "--noconfirm",
    "--onefile",
    "--name",
    "docling-pdf-parserd",
    "--distpath",
    distDir,
    "--workpath",
    workDir,
    "--specpath",
    workDir,
    "--paths",
    resolve(projectDir, "document-parser"),
    "--collect-all",
    "docling",
    "--collect-all",
    "docling_core",
    "--collect-all",
    "docling_ibm_models",
    "--collect-all",
    "docling_parse",
    "--collect-all",
    "rapidocr",
    "--collect-all",
    "onnxruntime",
    parserEntry,
  ],
  pdfDir,
);

const extension = target.includes("windows") ? ".exe" : "";
const parserBinary = resolve(distDir, `docling-pdf-parserd${extension}`);
if (!existsSync(parserBinary)) throw new Error(`Missing parser binary: ${parserBinary}`);
const packagePath = resolve(packageDir, `docling-pdf-local-1-${target}.zip`);
run(
  "uv",
  [
    "run",
    "python",
    "build_package.py",
    "--parser",
    parserBinary,
    "--models",
    modelsDir,
    "--target",
    target,
    "--output",
    packagePath,
  ],
  pdfDir,
);
