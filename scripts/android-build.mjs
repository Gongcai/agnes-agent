import { existsSync, readdirSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import { spawnSync } from "node:child_process";

const sdkRoot = process.env.ANDROID_SDK_ROOT || process.env.ANDROID_HOME || join(homedir(), "Android", "Sdk");

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
  PATH: `${ndkBin}${delimiter}${process.env.PATH || ""}`,
};

for (const [target, [cargoTarget, compiler]] of Object.entries(toolchains)) {
  const compilerPath = join(ndkBin, compiler);
  if (!existsSync(compilerPath)) throw new Error(`Missing Android compiler: ${compilerPath}`);
  env[`CC_${cargoTarget}`] = compilerPath;
  env[`CXX_${cargoTarget}`] = `${compilerPath}++`;
  env[`CARGO_TARGET_${cargoTarget.toUpperCase()}_LINKER`] = compilerPath;
}

const args = process.argv.slice(2).filter((value, index) => !(index === 0 && value === "--"));
const buildArgs = args.length === 0 ? ["--debug", "--apk", "--target", "aarch64"] : args;
const result = spawnSync("pnpm", ["tauri", "android", "build", ...buildArgs], {
  env,
  stdio: "inherit",
});

if (result.error) throw result.error;
process.exit(result.status ?? 1);
