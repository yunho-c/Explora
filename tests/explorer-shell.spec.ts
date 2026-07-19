import { expect, test } from "@playwright/test";

test("navigates the demo shell and opens Quick Preview", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("main", { name: "File explorer" })).toBeVisible();
  await expect(page.getByText("explora-notes.md")).toBeVisible();

  await page.getByText("explora-notes.md").click();
  await page.keyboard.press("Space");
  await expect(page.getByRole("dialog")).toContainText("explora-notes.md");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toBeHidden();

  await page.getByRole("button", { name: "Grid view" }).click();
  await expect(page.getByRole("grid", { name: "Files" })).toBeVisible();
});

test("filters entries and follows a dark system preference", async ({
  page,
}) => {
  await page.emulateMedia({ colorScheme: "dark" });
  await page.goto("/");

  await expect(page.locator("html")).toHaveClass(/dark/);
  await expect(page.getByText("explora-notes.md")).toBeVisible();
  await page
    .getByRole("textbox", { name: "Search this location" })
    .fill("summer");
  await expect(page.getByText("summer-light.jpg")).toBeVisible();
  await expect(page.getByText("explora-notes.md")).toBeHidden();
});

test("opens a folder and returns to its parent", async ({ page }) => {
  await page.goto("/");

  await page.getByText("Projects", { exact: true }).dblclick();
  await expect(page.getByText("This location is empty")).toBeVisible();
  await page.getByRole("button", { name: "Go to parent folder" }).click();
  await expect(page.getByText("explora-notes.md")).toBeVisible();
});

test("opens the responsive locations sheet", async ({ page }) => {
  await page.setViewportSize({ width: 760, height: 720 });
  await page.goto("/");

  await page.getByRole("button", { name: "Open locations" }).click();
  await expect(page.getByRole("dialog")).toContainText(
    "Choose a favorite or saved location.",
  );
  await page
    .getByRole("dialog")
    .getByRole("button", { name: /staging-box/ })
    .click();
  await expect(page.getByText("service.log")).toBeVisible();
});
