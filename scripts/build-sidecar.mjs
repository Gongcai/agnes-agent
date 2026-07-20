import { chmodSync, copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const projectDir = resolve(scriptDir, "..");
const agentDir = resolve(projectDir, "agent");
const documentParserDir = resolve(projectDir, "document-parser");
const binaryDir = resolve(projectDir, "src-tauri", "binaries");

function commandOutput(command, args) {
  const result = spawnSync(command, args, {
    cwd: projectDir,
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw result.error || new Error(result.stderr || `${command} exited with status ${result.status}`);
  }
  return result.stdout;
}

function rustHostTriple() {
  const match = commandOutput("rustc", ["-vV"]).match(/^host:\s*(\S+)$/m);
  if (!match) throw new Error("Unable to determine the Rust host target triple");
  return match[1];
}

function argumentValue(name) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : undefined;
}

const target =
  argumentValue("--target") || process.env.TAURI_ENV_TARGET_TRIPLE || rustHostTriple();
const host = rustHostTriple();
const isWindowsTarget = target.includes("windows");
const targetExtension = isWindowsTarget ? ".exe" : "";
const destination = resolve(binaryDir, `agentd-${target}${targetExtension}`);
const suppliedBinary = process.env.AGNES_SIDECAR_BINARY;
const suppliedDocumentParserBinary = process.env.AGNES_DOCUMENT_PARSER_BINARY;

mkdirSync(binaryDir, { recursive: true });

if (suppliedBinary) {
  const source = resolve(suppliedBinary);
  if (!existsSync(source)) throw new Error(`AGNES_SIDECAR_BINARY does not exist: ${source}`);
  if (source !== destination) copyFileSync(source, destination);
} else {
  if (target !== host) {
    throw new Error(
      `PyInstaller cannot cross-compile from ${host} to ${target}. ` +
        "Build on the target platform or set AGNES_SIDECAR_BINARY to a prebuilt executable.",
    );
  }

  const distDir = resolve(agentDir, ".sidecar-dist", target);
  const workDir = resolve(agentDir, ".sidecar-build", target);
  const pyInstallerArgs = [
    "run",
    "python",
    "-m",
    "PyInstaller",
    "--clean",
    "--noconfirm",
    "--onefile",
    "--name",
    "agentd",
    "--distpath",
    distDir,
    "--workpath",
    workDir,
    "--specpath",
    workDir,
    "--collect-all",
    "litellm",
    "--collect-all",
    "langgraph",
    "--collect-all",
    "langchain_core",
    "--collect-submodules",
    "tiktoken_ext",
    "agentd.py",
  ];
  const result = spawnSync("uv", pyInstallerArgs, {
    cwd: agentDir,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    throw new Error(`PyInstaller exited with status ${result.status}`);
  }

  const builtBinary = resolve(distDir, `agentd${targetExtension}`);
  if (!existsSync(builtBinary)) {
    throw new Error(`PyInstaller did not produce the expected binary: ${builtBinary}`);
  }
  copyFileSync(builtBinary, destination);
}

if (!isWindowsTarget) chmodSync(destination, 0o755);
console.log(`Prepared Tauri sidecar: ${destination}`);

const parserDestination = resolve(
  binaryDir,
  `document-parserd-${target}${targetExtension}`,
);

if (suppliedDocumentParserBinary) {
  const source = resolve(suppliedDocumentParserBinary);
  if (!existsSync(source)) {
    throw new Error(`AGNES_DOCUMENT_PARSER_BINARY does not exist: ${source}`);
  }
  if (source !== parserDestination) copyFileSync(source, parserDestination);
} else {
  if (target !== host) {
    throw new Error(
      `PyInstaller cannot cross-compile document-parserd from ${host} to ${target}. ` +
        "Build on the target platform or set AGNES_DOCUMENT_PARSER_BINARY.",
    );
  }

  const parserDistDir = resolve(documentParserDir, ".sidecar-dist", target);
  const parserWorkDir = resolve(documentParserDir, ".sidecar-build", target);
  const parserArgs = [
    "run",
    "python",
    "-m",
    "PyInstaller",
    "--clean",
    "--noconfirm",
    "--onefile",
    "--name",
    "document-parserd",
    "--distpath",
    parserDistDir,
    "--workpath",
    parserWorkDir,
    "--specpath",
    parserWorkDir,
    "--collect-all",
    "docling_core",
    "--collect-data",
    "docling",
    "--collect-data",
    "docx",
    "--collect-data",
    "pptx",
    "--hidden-import",
    "docling.backend.msword_backend",
    "--hidden-import",
    "docling.backend.mspowerpoint_backend",
    "--hidden-import",
    "docling.backend.msexcel_backend",
    "document_parserd.py",
  ];
  const result = spawnSync("uv", parserArgs, {
    cwd: documentParserDir,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    throw new Error(`document-parserd PyInstaller exited with status ${result.status}`);
  }

  const builtBinary = resolve(parserDistDir, `document-parserd${targetExtension}`);
  if (!existsSync(builtBinary)) {
    throw new Error(`PyInstaller did not produce the expected binary: ${builtBinary}`);
  }
  copyFileSync(builtBinary, parserDestination);
}

if (!isWindowsTarget) chmodSync(parserDestination, 0o755);
console.log(`Prepared document parser sidecar: ${parserDestination}`);

if (target === host) {
  const smokeTest = spawnSync(
    "uv",
    ["run", "python", "tests/smoke_sidecar.py", destination],
    { cwd: agentDir, stdio: "inherit" },
  );
  if (smokeTest.status !== 0) {
    throw new Error(`Frozen sidecar smoke test exited with status ${smokeTest.status}`);
  }
  const parserSmokeTest = spawnSync(
    "uv",
    ["run", "python", "tests/smoke_parser.py", parserDestination],
    { cwd: documentParserDir, stdio: "inherit" },
  );
  if (parserSmokeTest.status !== 0) {
    throw new Error(
      `Frozen document parser smoke test exited with status ${parserSmokeTest.status}`,
    );
  }
}
