import { existsSync, readFileSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const sdkRoot = process.env.ANDROID_SDK_ROOT || process.env.ANDROID_HOME || join(homedir(), "Android", "Sdk");
const scriptDir = dirname(fileURLToPath(import.meta.url));
const tauriCli = join(scriptDir, "..", "node_modules", ".bin", process.platform === "win32" ? "tauri.cmd" : "tauri");

function findJavaHome() {
  const candidates = [
    process.env.JAVA_HOME,
    "/usr/lib/jvm/java-21-openjdk",
    "/usr/lib/jvm/java-17-openjdk",
    "/usr/lib/jvm/default",
  ].filter(Boolean);
  for (const candidate of candidates) {
    const compiler = join(candidate, "bin", process.platform === "win32" ? "javac.exe" : "javac");
    const release = join(candidate, "release");
    if (!existsSync(compiler) || !existsSync(release)) continue;
    const major = Number(readFileSync(release, "utf8").match(/^JAVA_VERSION="(\d+)/m)?.[1]);
    if (major >= 17) return candidate;
  }
  throw new Error("Android builds require a full JDK 17 or newer; set JAVA_HOME to its installation directory");
}

function findNdk() {
  const configured = [process.env.ANDROID_NDK_HOME, process.env.NDK_HOME].filter(Boolean);
  for (const candidate of configured) {
    if (existsSync(join(candidate, "toolchains", "llvm"))) return candidate;
  }

  const ndkRoot = join(sdkRoot, "ndk");
  if (!existsSync(ndkRoot)) throw new Error(`Android NDK directory does not exist: ${ndkRoot}`);
  const version = readdirSync(ndkRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort()
    .reverse()
    .find((entry) => existsSync(join(ndkRoot, entry, "toolchains", "llvm")));
  if (!version) throw new Error(`No installed Android NDK found under ${ndkRoot}`);
  return join(ndkRoot, version);
}

const ndkRoot = findNdk();
const javaHome = findJavaHome();
const hostPrebuilt = process.platform === "linux" ? "linux-x86_64" : "darwin-x86_64";
const ndkBin = join(ndkRoot, "toolchains", "llvm", "prebuilt", hostPrebuilt, "bin");
if (!existsSync(ndkBin)) throw new Error(`Android NDK LLVM toolchain does not exist: ${ndkBin}`);

const toolchains = {
  aarch64: ["aarch64_linux_android", "aarch64-linux-android24-clang"],
  armv7: ["armv7_linux_androideabi", "armv7a-linux-androideabi24-clang"],
  i686: ["i686_linux_android", "i686-linux-android24-clang"],
  x86_64: ["x86_64_linux_android", "x86_64-linux-android24-clang"],
};

const env = {
  ...process.env,
  ANDROID_HOME: sdkRoot,
  ANDROID_SDK_ROOT: sdkRoot,
  ANDROID_NDK_HOME: ndkRoot,
  NDK_HOME: ndkRoot,
  JAVA_HOME: javaHome,
  PATH: `${join(javaHome, "bin")}${delimiter}${ndkBin}${delimiter}${process.env.PATH || ""}`,
};

for (const [target, [cargoTarget, compiler]] of Object.entries(toolchains)) {
  const compilerPath = join(ndkBin, compiler);
  if (!existsSync(compilerPath)) throw new Error(`Missing Android compiler: ${compilerPath}`);
  env[`CC_${cargoTarget}`] = compilerPath;
  env[`CXX_${cargoTarget}`] = `${compilerPath}++`;
  env[`CARGO_TARGET_${cargoTarget.toUpperCase()}_LINKER`] = compilerPath;
}

const args = process.argv.slice(2).filter((value, index) => !(index === 0 && value === "--"));
// Tauri Android builds are release by default. Accept the conventional flag
// at this wrapper boundary without forwarding it to the Tauri CLI.
const releaseRequested = args.includes("--release");
const buildArgs = args
  .filter((value) => value !== "--release")
  .concat(args.length === 0 ? ["--debug", "--apk", "--target", "aarch64"] : []);
if (releaseRequested && buildArgs.length === 0) buildArgs.push("--apk", "--target", "aarch64");
const informational = buildArgs.some((value) => value === "--help" || value === "-h" || value === "--version" || value === "-V");
if (!informational && !buildArgs.some((value) => value === "--target" || value === "-t" || value.startsWith("--target=") || value.startsWith("-t="))) {
  buildArgs.push("--target", "aarch64");
}

function targetArgument(values) {
  const inline = values.find((value) => value.startsWith("--target=") || value.startsWith("-t="));
  if (inline) return inline.slice(inline.indexOf("=") + 1);
  const index = Math.max(values.lastIndexOf("--target"), values.lastIndexOf("-t"));
  if (index < 0) return "aarch64";
  const targets = [];
  for (let cursor = index + 1; cursor < values.length && !values[cursor].startsWith("-"); cursor += 1) {
    targets.push(values[cursor]);
  }
  if (targets.length !== 1) {
    throw new Error("Android Runtime builds require exactly one --target ABI");
  }
  return targets[0];
}

const prepareArgs = [
  join(scriptDir, "prepare-android-runtime-assets.mjs"),
  "--target",
  targetArgument(buildArgs),
];
if (!informational) {
  if (process.env.AGNES_ANDROID_RUNTIME_OPTIONAL === "1") prepareArgs.push("--optional");
  const prepared = spawnSync(process.execPath, prepareArgs, {
    env,
    stdio: "inherit",
  });
  if (prepared.error) throw prepared.error;
  if (prepared.status !== 0) process.exit(prepared.status ?? 1);
}

if (!existsSync(tauriCli)) throw new Error(`Local Tauri CLI does not exist: ${tauriCli}`);
const result = spawnSync(tauriCli, ["android", "build", ...buildArgs], {
  env,
  stdio: "inherit",
});

if (result.error) throw result.error;
process.exit(result.status ?? 1);
