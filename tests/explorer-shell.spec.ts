import { expect, test } from "@playwright/test";

test("navigates the demo shell and opens Quick Preview", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByRole("main", { name: "File explorer" })).toBeVisible();
  await expect(page.getByText("explora-notes.md")).toBeVisible();

  await page.getByText("explora-notes.md").click();
  await page.keyboard.press("Space");
  await expect(page.getByRole("dialog")).toContainText("explora-notes.md");
  await expect(
    page.getByRole("textbox", {
      name: "Text preview of explora-notes.md",
    }),
  ).toHaveValue(/local and remote files/);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog")).toBeHidden();

  await page.getByText("summer-light.jpg").click();
  await page.keyboard.press("Space");
  await expect(
    page.getByRole("img", { name: "Preview of summer-light.jpg" }),
  ).toBeVisible();
  const sanitizeToggle = page.getByRole("button", {
    name: "Use sanitized image preview",
  });
  await expect(sanitizeToggle).toHaveAttribute("aria-pressed", "false");
  await sanitizeToggle.focus();
  await page.keyboard.press("Space");
  await expect(
    page.getByRole("button", { name: "Use direct image preview" }),
  ).toHaveAttribute("aria-pressed", "true");
  await expect(
    page.getByRole("img", { name: "Preview of summer-light.jpg" }),
  ).toBeVisible();
  await page.keyboard.press("Escape");

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
    .getByRole("button", { name: "staging-box connected" })
    .click();
  await expect(page.getByText("service.log")).toBeVisible();
});

test("adds an SSH target and connects a configured demo remote", async ({
  page,
}) => {
  await page.goto("/");

  await page.getByRole("button", { name: "Add SSH target" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox", { name: "Name", exact: true }).fill("Lab");
  await dialog.getByRole("textbox", { name: "Host" }).fill("lab.example.com");
  await dialog.getByRole("textbox", { name: "Username" }).fill("yunho");
  await dialog.getByRole("button", { name: "Save target" }).click();
  await expect(
    page.getByRole("button", { name: "Lab disconnected" }),
  ).toBeVisible();

  await page.getByRole("button", { name: /render-node Config/ }).click();
  await expect(page.getByRole("tab", { name: "render-node" })).toBeVisible();
  await expect(page.getByText("This location is empty")).toBeVisible();
});

test("preserves an SSH tab through disconnect, reconnect, and refresh", async ({
  page,
}) => {
  await page.goto("/");

  await page.getByRole("button", { name: /render-node Config/ }).click();
  const remoteTab = page.getByRole("tab", { name: "render-node" });
  await expect(remoteTab).toBeVisible();

  await page.getByRole("button", { name: "Manage render-node" }).click();
  await page.getByRole("menuitem", { name: "Disconnect" }).click();
  await expect(page.getByText("Remote location is offline")).toBeVisible();
  await expect(remoteTab).toBeVisible();

  await page.getByRole("button", { name: "Reconnect" }).click();
  await expect(page.getByText("Remote location is offline")).toBeHidden();
  await expect(remoteTab).toBeVisible();

  await page.getByRole("button", { name: "Refresh current folder" }).click();
  await expect(page.getByText("This location is empty")).toBeVisible();
  await expect(remoteTab).toBeVisible();
});
