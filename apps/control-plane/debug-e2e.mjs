import { chromium } from "@playwright/test";
const browser = await chromium.launch();
const page = await browser.newPage();
page.on("console", (m) => console.log("[c:" + m.type() + "]", m.text().slice(0, 600)));
page.on("pageerror", (e) => console.log("[pageerror]", String(e).slice(0, 900)));
await page.goto("http://127.0.0.1:3100/", { waitUntil: "networkidle", timeout: 90000 });
await page.waitForTimeout(2500);
await browser.close();
