import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:3000",
    trace: "retain-on-failure",
    ...devices["Desktop Chrome"],
  },
  webServer: {
    // E2E runs against a real production build (`next build && next start`),
    // not the dev server: prod mode serves the exact bundles CI ships and
    // avoids dev-only HMR/WebSocket dependencies in headless sandboxes.
    command: "npm run build && npm run start -- -p 3000 -H 0.0.0.0",
    url: "http://127.0.0.1:3000",
    reuseExistingServer: !process.env.CI,
    timeout: 240_000,
    env: {
      NEXT_PUBLIC_API_ORIGIN: "",
      NEXT_TELEMETRY_DISABLED: "1",
    },
  },
});
