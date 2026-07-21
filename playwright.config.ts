import { defineConfig, devices } from "@playwright/test";

const requestedPort = process.env.EXPLORA_E2E_PORT ?? "6750";
const e2ePort = Number(requestedPort);
if (
  !/^\d+$/.test(requestedPort) ||
  !Number.isInteger(e2ePort) ||
  e2ePort < 1_024 ||
  e2ePort > 65_535
) {
  throw new Error("EXPLORA_E2E_PORT must be between 1024 and 65535.");
}
const e2eUrl = `http://127.0.0.1:${e2ePort}`;

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  reporter: "list",
  use: {
    baseURL: e2eUrl,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        viewport: { width: 1280, height: 820 },
      },
    },
  ],
  webServer: {
    command: `bun run dev:web -- --port ${e2ePort}`,
    url: e2eUrl,
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
