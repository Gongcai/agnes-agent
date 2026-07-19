import { chmodSync, copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const projectDir = resolve(scriptDir, "..");
const agentDir = resolve(projectDir, "agent");
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

if (target === host) {
  const smokeTest = spawnSync(
    "uv",
    ["run", "python", "tests/smoke_sidecar.py", destination],
    { cwd: agentDir, stdio: "inherit" },
  );
  if (smokeTest.status !== 0) {
    throw new Error(`Frozen sidecar smoke test exited with status ${smokeTest.status}`);
  }
}
