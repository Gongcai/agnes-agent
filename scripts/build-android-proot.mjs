import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const projectDir = resolve(scriptDir, "..");
const sourceLockPath = join(projectDir, "android-runtime", "sources.lock.json");
const sourceLock = JSON.parse(readFileSync(sourceLockPath, "utf8"));

function argumentValue(name, fallback) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : fallback;
}

function hasArgument(name) {
  return process.argv.includes(name);
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: options.cwd,
    env: options.env || process.env,
    encoding: options.capture ? "utf8" : undefined,
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    const details = options.capture ? `\n${result.stderr || result.stdout}` : "";
    throw new Error(`${command} exited with status ${result.status}${details}`);
  }
  return options.capture ? result.stdout.trim() : "";
}

function requireTool(name) {
  run("sh", ["-c", `command -v "$1" >/dev/null`, "require-tool", name]);
}

function findNdk() {
  for (const candidate of [process.env.ANDROID_NDK_HOME, process.env.NDK_HOME]) {
    if (candidate && existsSync(join(candidate, "toolchains", "llvm"))) return resolve(candidate);
  }
  const sdkRoot = process.env.ANDROID_SDK_ROOT || process.env.ANDROID_HOME || join(homedir(), "Android", "Sdk");
  const ndkRoot = join(sdkRoot, "ndk");
  if (!existsSync(ndkRoot)) throw new Error(`Android NDK directory does not exist: ${ndkRoot}`);
  const version = readdirSync(ndkRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort()
    .reverse()
    .find((entry) => existsSync(join(ndkRoot, entry, "toolchains", "llvm")));
  if (!version) throw new Error(`No Android NDK found under ${ndkRoot}`);
  return join(ndkRoot, version);
}

function cloneLockedSource(source, destination, repositoryOverride, sparsePaths = []) {
  const repository = repositoryOverride || source.repository;
  run("git", ["clone", "--depth", "1", "--filter=blob:none", "--no-checkout", "--branch", source.tag, repository, destination]);
  if (sparsePaths.length > 0) {
    run("git", ["sparse-checkout", "set", ...sparsePaths], { cwd: destination });
  }
  run("git", ["checkout", "--detach", source.commit], { cwd: destination });
  const actualCommit = run("git", ["rev-parse", "HEAD"], { cwd: destination, capture: true });
  if (actualCommit !== source.commit) {
    throw new Error(`Source commit mismatch for ${repository}: expected ${source.commit}, got ${actualCommit}`);
  }
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function inspectElf(path, expectedMachine, readelf) {
  const header = run(readelf, ["-h", path], { capture: true });
  const programHeaders = run(readelf, ["-l", path], { capture: true });
  const dynamic = run(readelf, ["-d", path], { capture: true });
  if (
    !header.includes("ELF64")
    || !header.includes("Type:")
    || !header.includes("DYN")
    || /Entry point address:\s+0x0(?:\s|$)/.test(header)
  ) {
    throw new Error("PRoot output is not a 64-bit PIE executable");
  }
  if (!header.includes(expectedMachine)) {
    throw new Error(`PRoot output has the wrong machine type; expected ${expectedMachine}`);
  }
  if (!programHeaders.includes("/system/bin/linker64")) {
    throw new Error("PRoot output does not use the Android 64-bit linker");
  }
  if (dynamic.includes("(SONAME)")) {
    throw new Error("PRoot output is a shared library instead of a PIE executable");
  }
  const dependencies = [...dynamic.matchAll(/Shared library: \[([^\]]+)\]/g)].map((match) => match[1]);
  const unexpected = dependencies.filter((name) => name !== "libc.so" && name !== "libdl.so");
  if (unexpected.length > 0) {
    throw new Error(`PRoot output has unexpected dynamic dependencies: ${unexpected.join(", ")}`);
  }
  return dependencies;
}

function inspectLoaderElf(path, expectedMachine, readelf) {
  const header = run(readelf, ["-h", path], { capture: true });
  const programHeaders = run(readelf, ["-l", path], { capture: true });
  const dynamic = run(readelf, ["-d", path], { capture: true });
  if (
    !header.includes("ELF64")
    || !header.includes("Type:")
    || !header.includes("EXEC")
    || /Entry point address:\s+0x0(?:\s|$)/.test(header)
  ) {
    throw new Error("PRoot loader output is not a 64-bit executable");
  }
  if (!header.includes(expectedMachine)) {
    throw new Error(`PRoot loader output has the wrong machine type; expected ${expectedMachine}`);
  }
  if (programHeaders.includes("INTERP") || dynamic.includes("(NEEDED)")) {
    throw new Error("PRoot loader output is not statically linked");
  }
}

function writeSbom(outputDir, abi, binaryPath, loaderPath, dependencies) {
  const created = new Date(sourceLock.sourceDateEpoch * 1000).toISOString().replace(".000Z", "Z");
  const namespaceSeed = `${sourceLock.proot.commit}:${sourceLock.talloc.commit}:${abi}:${sha256(binaryPath)}:${sha256(loaderPath)}`;
  const namespace = createHash("sha256").update(namespaceSeed).digest("hex");
  const packages = [sourceLock.proot, sourceLock.talloc].map((source, index) => ({
    SPDXID: index === 0 ? "SPDXRef-Package-PRoot" : "SPDXRef-Package-talloc",
    name: index === 0 ? "Termux PRoot" : "talloc",
    versionInfo: source.version,
    downloadLocation: `${source.repository}@${source.commit}`,
    filesAnalyzed: false,
    licenseConcluded: source.license,
    licenseDeclared: source.license,
    supplier: index === 0 ? "Organization: Termux" : "Organization: Samba Team",
    externalRefs: [{
      referenceCategory: "PACKAGE-MANAGER",
      referenceType: "purl",
      referenceLocator: `pkg:github/${index === 0 ? "termux/proot" : "samba-team/samba"}@${source.commit}`,
    }],
  }));
  const document = {
    spdxVersion: "SPDX-2.3",
    dataLicense: "CC0-1.0",
    SPDXID: "SPDXRef-DOCUMENT",
    name: `Agnes Android PRoot ${abi}`,
    documentNamespace: `https://agnes.local/spdx/android-proot/${namespace}`,
    creationInfo: { created, creators: ["Tool: scripts/build-android-proot.mjs"] },
    packages,
    files: [
      {
        SPDXID: "SPDXRef-File-libagnes-proot",
        fileName: "libagnes_proot.so",
        checksums: [{ algorithm: "SHA256", checksumValue: sha256(binaryPath) }],
        licenseConcluded: "GPL-2.0-only AND LGPL-3.0-or-later",
        licenseInfoInFiles: ["NOASSERTION"],
        copyrightText: "NOASSERTION",
        comment: `Android system dependencies: ${dependencies.join(", ")}`,
      },
      {
        SPDXID: "SPDXRef-File-libagnes-proot-loader",
        fileName: "libagnes_proot_loader.so",
        checksums: [{ algorithm: "SHA256", checksumValue: sha256(loaderPath) }],
        licenseConcluded: "GPL-2.0-only",
        licenseInfoInFiles: ["NOASSERTION"],
        copyrightText: "NOASSERTION",
        comment: "Statically linked PRoot ELF loader",
      },
    ],
    relationships: [
      { spdxElementId: "SPDXRef-DOCUMENT", relationshipType: "DESCRIBES", relatedSpdxElement: "SPDXRef-Package-PRoot" },
      { spdxElementId: "SPDXRef-Package-PRoot", relationshipType: "STATIC_LINK", relatedSpdxElement: "SPDXRef-Package-talloc" },
      { spdxElementId: "SPDXRef-Package-PRoot", relationshipType: "CONTAINS", relatedSpdxElement: "SPDXRef-File-libagnes-proot" },
      { spdxElementId: "SPDXRef-Package-PRoot", relationshipType: "CONTAINS", relatedSpdxElement: "SPDXRef-File-libagnes-proot-loader" },
    ],
  };
  writeFileSync(join(outputDir, "proot.spdx.json"), `${JSON.stringify(document, null, 2)}\n`);
}

if (hasArgument("--help") || hasArgument("-h")) {
  console.log(`Usage: node scripts/build-android-proot.mjs [options]

Options:
  --abi <x86_64|arm64-v8a>       Target Android ABI (default: arm64-v8a)
  --output-dir <path>            Artifact directory (default: android-runtime/<abi>)
  --work-dir <path>              Keep and use a specific build directory
  --proot-repository <url|path>  Override the locked PRoot repository URL
  --talloc-repository <url|path> Override the locked Samba repository URL
  --jobs <count>                 Parallel build jobs
`);
  process.exit(0);
}

const abi = argumentValue("--abi", "arm64-v8a");
const targets = {
  "arm64-v8a": { clang: "aarch64-linux-android", machine: "AArch64" },
  x86_64: { clang: "x86_64-linux-android", machine: "Advanced Micro Devices X86-64" },
};
const target = targets[abi];
if (!target) throw new Error(`Unsupported Android ABI: ${abi}`);

for (const tool of ["git", "make", "sh"]) requireTool(tool);
const ndkRoot = findNdk();
const hostTag = process.platform === "linux" ? "linux-x86_64" : "darwin-x86_64";
const toolchain = join(ndkRoot, "toolchains", "llvm", "prebuilt", hostTag, "bin");
const api = sourceLock.androidApi;
const cc = join(toolchain, `${target.clang}${api}-clang`);
const ar = join(toolchain, "llvm-ar");
const strip = join(toolchain, "llvm-strip");
const objcopy = join(toolchain, "llvm-objcopy");
const objdump = join(toolchain, "llvm-objdump");
const readelf = join(toolchain, "llvm-readelf");
for (const tool of [cc, ar, strip, objcopy, objdump, readelf]) {
  if (!existsSync(tool)) throw new Error(`Required NDK tool does not exist: ${tool}`);
}

const configuredWorkDir = argumentValue("--work-dir");
const workDir = configuredWorkDir ? resolve(configuredWorkDir) : mkdtempSync(join(tmpdir(), `agnes-proot-${abi}-`));
const outputDir = resolve(argumentValue("--output-dir", join(projectDir, "android-runtime", abi)));
const jobs = argumentValue("--jobs", String(Math.max(1, Math.min(8, Number(process.env.CI ? 2 : 4)))));
mkdirSync(workDir, { recursive: true });
mkdirSync(outputDir, { recursive: true });

try {
  const prootDir = join(workDir, "proot");
  const sambaDir = join(workDir, "samba");
  const prefixDir = join(workDir, "prefix");
  cloneLockedSource(sourceLock.proot, prootDir, argumentValue("--proot-repository"));
  cloneLockedSource(
    sourceLock.talloc,
    sambaDir,
    argumentValue("--talloc-repository"),
    ["lib/talloc", "lib/replace", "buildtools", "third_party/waf"],
  );

  for (const patch of sourceLock.proot.patches) {
    run("git", ["apply", join(projectDir, "android-runtime", patch)], { cwd: prootDir });
  }

  const tallocDir = join(sambaDir, "lib", "talloc");
  const crossAnswers = `Checking uname sysname type: "Linux"
Checking uname machine type: "dontcare"
Checking uname release type: "dontcare"
Checking uname version type: "dontcare"
Checking simple C program: OK
building library support: OK
Checking for large file support: OK
Checking for -D_FILE_OFFSET_BITS=64: OK
Checking for WORDS_BIGENDIAN: OK
Checking for C99 vsnprintf: OK
Checking for HAVE_SECURE_MKSTEMP: OK
rpath library support: OK
-Wl,--version-script support: FAIL
Checking correct behavior of strtoll: OK
Checking correct behavior of strptime: OK
Checking for HAVE_IFACE_GETIFADDRS: OK
Checking for HAVE_IFACE_IFCONF: OK
Checking for HAVE_IFACE_IFREQ: OK
Checking getconf LFS_CFLAGS: OK
Checking for large file support without additional flags: OK
Checking for working strptime: OK
Checking for HAVE_SHARED_MMAP: OK
Checking for HAVE_MREMAP: OK
Checking for HAVE_INCOHERENT_MMAP: OK
Checking getconf large file support flags work: OK
`;
  writeFileSync(join(tallocDir, "cross-answers.txt"), crossAnswers);
  mkdirSync(prefixDir, { recursive: true });
  const compileEnv = { ...process.env, CC: cc, AR: ar, SOURCE_DATE_EPOCH: String(sourceLock.sourceDateEpoch) };
  run("./configure", [
    `--prefix=${prefixDir}`,
    "--disable-rpath",
    "--disable-python",
    "--cross-compile",
    "--cross-answers=cross-answers.txt",
  ], { cwd: tallocDir, env: compileEnv });
  run("make", [`-j${jobs}`], { cwd: tallocDir, env: compileEnv });

  const tallocObjectsDir = join(tallocDir, "bin", "default", "lib", "talloc");
  const tallocObject = readdirSync(tallocObjectsDir)
    .filter((name) => /^talloc\.c\.\d+\.o$/.test(name))
    .sort((left, right) => Number(right.match(/\d+/)[0]) - Number(left.match(/\d+/)[0]))[0];
  if (!tallocObject) throw new Error("Unable to locate the Android talloc object file");
  mkdirSync(join(prefixDir, "include"), { recursive: true });
  mkdirSync(join(prefixDir, "lib"), { recursive: true });
  copyFileSync(join(tallocDir, "talloc.h"), join(prefixDir, "include", "talloc.h"));
  run(ar, ["rcs", join(prefixDir, "lib", "libtalloc.a"), join(tallocObjectsDir, tallocObject)]);

  const prootSourceDir = join(prootDir, "src");
  const makeVariables = [
    `CC=${cc}`,
    `STRIP=${strip}`,
    `OBJCOPY=${objcopy}`,
    `OBJDUMP=${objdump}`,
    `CPPFLAGS=-D_FILE_OFFSET_BITS=64 -D_GNU_SOURCE -I. -I${prootSourceDir} -I${join(prefixDir, "include")} -DARG_MAX=131072 -DVERSION=\\\"${sourceLock.proot.version}\\\"`,
    `LDFLAGS=-L${join(prefixDir, "lib")} -ltalloc -Wl,-z,noexecstack`,
  ];
  run("make", [`-j${jobs}`, ...makeVariables], {
    cwd: prootSourceDir,
    env: { ...process.env, SOURCE_DATE_EPOCH: String(sourceLock.sourceDateEpoch) },
  });

  const unstripped = join(prootSourceDir, "proot");
  const unstrippedLoader = join(prootSourceDir, "loader", "loader");
  const binaryPath = join(outputDir, "libagnes_proot.so");
  const loaderPath = join(outputDir, "libagnes_proot_loader.so");
  copyFileSync(unstripped, binaryPath);
  copyFileSync(unstrippedLoader, loaderPath);
  run(strip, ["--strip-unneeded", binaryPath]);
  run(strip, ["--strip-unneeded", loaderPath]);
  const dependencies = inspectElf(binaryPath, target.machine, readelf);
  inspectLoaderElf(loaderPath, target.machine, readelf);

  const licensesDir = join(outputDir, "licenses");
  mkdirSync(licensesDir, { recursive: true });
  copyFileSync(join(prootDir, "COPYING"), join(licensesDir, "PRoot-GPL-2.0.txt"));
  copyFileSync(join(tallocDir, "LICENSE"), join(licensesDir, "talloc-LGPL-3.0.txt"));
  copyFileSync(sourceLockPath, join(outputDir, "sources.lock.json"));
  writeSbom(outputDir, abi, binaryPath, loaderPath, dependencies);

  console.log(`Built Android PRoot: ${binaryPath}`);
  console.log(`SHA-256: ${sha256(binaryPath)}`);
  console.log(`Built Android PRoot loader: ${loaderPath}`);
  console.log(`Loader SHA-256: ${sha256(loaderPath)}`);
  console.log(`Dynamic dependencies: ${dependencies.join(", ")}`);
} finally {
  if (!configuredWorkDir) rmSync(workDir, { recursive: true, force: true });
}
