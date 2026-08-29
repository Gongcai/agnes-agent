import { cpSync, existsSync, mkdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const projectDir = resolve(scriptDir, "..");
const generatedMain = join(projectDir, "src-tauri", "gen", "android", "app", "src", "main");
const sourceRoot = resolve(process.env.AGNES_ANDROID_RUNTIME_DIR || join(projectDir, "android-runtime"));
const generatedAbis = ["arm64-v8a", "armeabi-v7a", "x86", "x86_64"];

function argumentValue(name, fallback) {
  const inline = process.argv.find((value) => value.startsWith(`${name}=`));
  if (inline) return inline.slice(name.length + 1);
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : fallback;
}

function normalizeTarget(value) {
  const target = value?.trim();
  if (["aarch64", "aarch64-linux-android", "arm64-v8a"].includes(target)) return "arm64-v8a";
  if (["x86_64", "x86_64-linux-android"].includes(target)) return "x86_64";
  return null;
}

function inferredTarget() {
  for (const value of [
    process.env.AGNES_ANDROID_TARGET,
    process.env.TAURI_ENV_ARCH,
    process.env.CARGO_CFG_TARGET_ARCH,
  ]) {
    const target = normalizeTarget(value);
    if (target) return target;
  }
  return null;
}

function assertFile(path, label) {
  if (!existsSync(path) || !statSync(path).isFile()) {
    throw new Error(`${label} does not exist: ${path}`);
  }
}

function validateZip(path) {
  const signature = readFileSync(path).subarray(0, 4);
  if (signature.length < 4 || signature[0] !== 0x50 || signature[1] !== 0x4b) {
    throw new Error(`Runtime package is not a ZIP file: ${path}`);
  }
}

function validateElf(path, abi) {
  const header = readFileSync(path).subarray(0, 20);
  if (header.length < 20) {
    throw new Error(`PRoot library has an incomplete ELF header: ${path}`);
  }
  const signature = header.subarray(0, 4);
  if (
    signature.length < 4 ||
    signature[0] !== 0x7f ||
    signature[1] !== 0x45 ||
    signature[2] !== 0x4c ||
    signature[3] !== 0x46
  ) {
    throw new Error(`PRoot library is not an ELF file: ${path}`);
  }
  const littleEndian = header[5] === 1;
  const machine = littleEndian ? header.readUInt16LE(18) : header.readUInt16BE(18);
  const expectedMachine = abi === "arm64-v8a" ? 183 : 62;
  if (machine !== expectedMachine) {
    throw new Error(`PRoot ELF ABI does not match ${abi}: ${path}`);
  }
}

function cleanGeneratedArtifacts(targetAbi) {
  rmSync(join(generatedMain, "assets", "agnes-runtime"), { recursive: true, force: true });
  for (const generatedAbi of generatedAbis) {
    rmSync(join(generatedMain, "jniLibs", generatedAbi, "libagnes_proot.so"), { force: true });
    if (generatedAbi !== targetAbi) {
      rmSync(join(generatedMain, "jniLibs", generatedAbi, "libagnes_agent_lib.so"), { force: true });
    }
  }
}

const requestedTarget = argumentValue("--target", "auto");
const abi = requestedTarget === "auto" ? inferredTarget() : normalizeTarget(requestedTarget);
const optional = process.argv.includes("--optional") || process.env.AGNES_ANDROID_RUNTIME_OPTIONAL === "1";
const dryRun = process.argv.includes("--dry-run");

if (!abi) {
  if (optional && requestedTarget === "auto") {
    console.warn("Skipping bundled Android Runtime staging because the target ABI is unavailable");
    process.exit(0);
  }
  throw new Error(`Unsupported or unavailable Android target: ${requestedTarget}`);
}

const runtimePackage = join(sourceRoot, abi, "runtime.zip");
const prootLibrary = join(sourceRoot, abi, "libagnes_proot.so");
const artifactsAvailable = existsSync(runtimePackage) && existsSync(prootLibrary);

if (!artifactsAvailable) {
  if (!optional) {
    throw new Error(
      `Bundled Android Runtime artifacts are incomplete for ${abi}. Expected ${runtimePackage} and ${prootLibrary}`,
    );
  }
  if (!dryRun) {
    cleanGeneratedArtifacts(abi);
  }
  console.warn(`Building ${abi} without a bundled Android Runtime`);
  process.exit(0);
}

assertFile(runtimePackage, "Runtime package");
assertFile(prootLibrary, "PRoot library");
validateZip(runtimePackage);
validateElf(prootLibrary, abi);

if (!dryRun) {
  cleanGeneratedArtifacts(abi);
  const assetDestination = join(generatedMain, "assets", "agnes-runtime", abi, "runtime.zip");
  const prootDestination = join(generatedMain, "jniLibs", abi, "libagnes_proot.so");
  mkdirSync(dirname(assetDestination), { recursive: true });
  mkdirSync(dirname(prootDestination), { recursive: true });
  cpSync(runtimePackage, assetDestination);
  cpSync(prootLibrary, prootDestination);
}

console.log(`${dryRun ? "Validated" : "Staged"} bundled Android Runtime for ${abi}`);
