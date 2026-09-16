const controller = new AbortController();
const timeout = setTimeout(() => controller.abort(), 4_000);

try {
  const response = await fetch("http://127.0.0.1:8787/v1/health", {
    signal: controller.signal,
  });
  if (response.status !== 200 && response.status !== 401) {
    process.exitCode = 1;
  }
} catch {
  process.exitCode = 1;
} finally {
  clearTimeout(timeout);
}
