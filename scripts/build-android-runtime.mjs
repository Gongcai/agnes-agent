import {
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  rmSync,
  chmodSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join, relative, resolve, sep } from "node:path";
import { tmpdir } from "node:os";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const projectDir = resolve(scriptDir, "..");
const packageJson = JSON.parse(readFileSync(join(projectDir, "package.json"), "utf8"));

function argumentValue(name, fallback) {
  const index = process.argv.indexOf(name);
  return index >= 0 ? process.argv[index + 1] : fallback;
}

function requiredArgument(name) {
  const value = argumentValue(name);
  if (!value) throw new Error(`Missing required argument ${name}`);
  return value;
}

function assertSafeVersion(value) {
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(value)) {
    throw new Error(`Invalid runtime version: ${value}`);
  }
}

function assertAbi(value) {
  if (value !== "arm64-v8a" && value !== "x86_64") {
    throw new Error(`Unsupported Android ABI: ${value}`);
  }
}

function copyTree(source, destination) {
  cpSync(source, destination, {
    recursive: true,
    dereference: false,
    verbatimSymlinks: true,
  });
}

function collectFiles(root) {
  const files = [];
  function visit(current) {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) {
        visit(path);
      } else if (entry.isFile() || entry.isSymbolicLink()) {
        files.push({ path, isSymlink: entry.isSymbolicLink() });
      } else {
        throw new Error(`Runtime rootfs contains unsupported entry: ${path}`);
      }
    }
  }
  visit(root);
  return files.sort((left, right) => {
    const a = relative(root, left.path).split(sep).join("/");
    const b = relative(root, right.path).split(sep).join("/");
    return Buffer.from(a).compare(Buffer.from(b));
  });
}

function digestRootfs(root) {
  const hash = createHash("sha256");
  let size = 0;
  for (const entry of collectFiles(root)) {
    const relativePath = relative(root, entry.path).split(sep).join("/");
    const bytes = entry.isSymlink
      ? Buffer.from(readlinkSync(entry.path))
      : readFileSync(entry.path);
    hash.update(relativePath);
    hash.update(Buffer.from([0]));
    hash.update(bytes);
    size += bytes.length;
  }
  return { sha256: hash.digest("hex"), size };
}

function runZip(output, packageRoot) {
  const result = spawnSync("zip", ["-rq", "-X", "-y", output, "manifest.json", "rootfs"], {
    cwd: packageRoot,
    stdio: "inherit",
  });
  if (result.status !== 0) throw new Error(`zip exited with status ${result.status}`);
}

function resetBuildHostResolver(rootfs) {
  const resolver = join(rootfs, "etc", "resolv.conf");
  mkdirSync(dirname(resolver), { recursive: true });
  try {
    if (lstatSync(resolver).isSymbolicLink()) unlinkSync(resolver);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  writeFileSync(
    resolver,
    "# Runtime resolver is supplied by Android ConnectivityManager at launch.\n",
  );
}

function resolveRootfsPath(rootfs, entryPath) {
  const root = resolve(rootfs);
  let current = resolve(root, entryPath);
  const seen = new Set();
  for (let depth = 0; depth < 40; depth += 1) {
    const relativePath = relative(root, current);
    if (
      relativePath.startsWith(`..${sep}`) ||
      relativePath === ".." ||
      relativePath.includes(`${sep}..${sep}`)
    ) {
      throw new Error(`Rootfs symlink escapes rootfs: ${entryPath}`);
    }
    const stat = lstatSync(current);
    if (!stat.isSymbolicLink()) return current;
    if (seen.has(current)) throw new Error(`Rootfs symlink loop: ${entryPath}`);
    seen.add(current);
    const target = readlinkSync(current);
    current = target.startsWith(sep)
      ? resolve(root, `.${target}`)
      : resolve(dirname(current), target);
  }
  throw new Error(`Rootfs symlink chain is too deep: ${entryPath}`);
}

function normalizePythonEntrypoints(rootfs) {
  const bin = join(rootfs, "usr", "bin");
  const python = join(bin, "python3");
  const pip312 = join(bin, "pip3.12");
  if (!existsSync(python)) {
    throw new Error(`Rootfs is missing required file: ${python}`);
  }
  if (!existsSync(pip312)) {
    throw new Error(`Rootfs is missing required file: ${pip312}`);
  }

  const resolvedPython = resolveRootfsPath(rootfs, "usr/bin/python3");
  const resolvedPip = resolveRootfsPath(rootfs, "usr/bin/pip3.12");
  if (!lstatSync(resolvedPython).isFile()) {
    throw new Error("Rootfs python3 does not resolve to a regular file");
  }
  if (!lstatSync(resolvedPip).isFile()) {
    throw new Error("Rootfs pip3.12 does not resolve to a regular file");
  }
  chmodSync(resolvedPython, 0o755);
  chmodSync(resolvedPip, 0o755);

  // Some Ubuntu rootfs exports contain pip3 -> pip3 (a broken self-link).
  // Normalize aliases in the copied package so the installed Runtime never
  // carries an ELOOP entrypoint.
  for (const [alias, target] of [["pip3", "pip3.12"], ["pip", "pip3"]]) {
    const aliasPath = join(bin, alias);
    let valid = false;
    try {
      valid =
        lstatSync(aliasPath).isFile() &&
        lstatSync(resolveRootfsPath(rootfs, `usr/bin/${alias}`)).isFile();
    } catch (error) {
      if (error.code !== "ENOENT" && !String(error.message).includes("symlink")) throw error;
    }
    if (!valid) {
      try {
        unlinkSync(aliasPath);
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
      symlinkSync(target, aliasPath);
    }
  }
  resolveRootfsPath(rootfs, "usr/bin/pip3");
  resolveRootfsPath(rootfs, "usr/bin/pip");
}

function ensureRuntimeDirectories(rootfs) {
  mkdirSync(join(rootfs, "dev", "shm"), { recursive: true });
  mkdirSync(join(rootfs, "tmp"), { recursive: true });
}

function ensureNoopSudo(rootfs) {
  const sudo = join(rootfs, "usr", "bin", "sudo");
  if (existsSync(sudo)) return;
  mkdirSync(dirname(sudo), { recursive: true });
  writeFileSync(sudo, "#!/bin/sh\nexec \"$@\"\n");
  chmodSync(sudo, 0o755);
}

const rootfsInput = resolve(requiredArgument("--rootfs"));
const abi = argumentValue("--abi", "arm64-v8a");
const runtimeVersion = argumentValue("--runtime-version", "ubuntu-24.04.1");
const agentVersion = argumentValue("--agent-version", packageJson.version);
const output = resolve(
  argumentValue(
    "--output",
    join(projectDir, "android-runtime", abi, "runtime.zip"),
  ),
);

assertAbi(abi);
assertSafeVersion(runtimeVersion);
if (!existsSync(rootfsInput) || !lstatSync(rootfsInput).isDirectory()) {
  throw new Error(`Rootfs directory does not exist: ${rootfsInput}`);
}
for (const requiredPath of [
  join(rootfsInput, "usr", "bin", "python3"),
]) {
  if (!existsSync(requiredPath)) {
    throw new Error(`Rootfs is missing required file: ${requiredPath}`);
  }
}

const workDir = mkdtempSync(join(tmpdir(), "agnes-android-runtime-"));
try {
  const packageRoot = join(workDir, "package");
  const rootfs = join(packageRoot, "rootfs");
  mkdirSync(rootfs, { recursive: true });
  copyTree(rootfsInput, rootfs);
  ensureRuntimeDirectories(rootfs);
  normalizePythonEntrypoints(rootfs);
  ensureNoopSudo(rootfs);
  resetBuildHostResolver(rootfs);

  const agentDestination = join(rootfs, "opt", "agnes-agent", "app");
  mkdirSync(dirname(agentDestination), { recursive: true });
  copyTree(join(projectDir, "agent", "app"), agentDestination);
  const promptDestination = join(rootfs, "opt", "agnes-agent", "ProjectPlan");
  mkdirSync(promptDestination, { recursive: true });
  cpSync(
    join(projectDir, "ProjectPlan", "MEMORY_FILESYSTEM_PROMPT.md"),
    join(promptDestination, "MEMORY_FILESYSTEM_PROMPT.md"),
  );

  const digest = digestRootfs(rootfs);
  const manifest = {
    schemaVersion: 1,
    runtimeVersion,
    platform: "android",
    abi,
    rootfsSha256: digest.sha256,
    rootfsSize: digest.size,
    agentVersion,
  };
  writeFileSync(join(packageRoot, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);

  mkdirSync(dirname(output), { recursive: true });
  if (existsSync(output)) unlinkSync(output);
  runZip(output, packageRoot);
  console.log(`Prepared Android Runtime: ${output}`);
  console.log(`Rootfs SHA-256: ${digest.sha256}`);
  console.log(`Rootfs bytes: ${digest.size}`);
} finally {
  rmSync(workDir, { recursive: true, force: true });
}
