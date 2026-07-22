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

test("renames a selected local entry inline", async ({ page }) => {
  await page.goto("/");

  await page.getByText("explora-notes.md").click();
  await page.keyboard.press("F2");
  const editor = page.getByRole("textbox", {
    name: "Rename explora-notes.md",
  });
  await expect(editor).toBeFocused();
  await editor.fill("renamed-notes.md");
  await editor.press("Enter");

  await expect(page.getByText("renamed-notes.md")).toBeVisible();
  await expect(editor).toBeHidden();
});

test("moves a local entry with the destination chooser", async ({ page }) => {
  await page.goto("/");

  await page.getByText("explora-notes.md").click({ button: "right" });
  await page.getByRole("menuitem", { name: "Move…" }).click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("heading", { name: "Move “explora-notes.md”" }),
  ).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
  await expect(
    dialog.getByRole("button", { name: "Move Here" }),
  ).toBeDisabled();
  await dialog.getByRole("button", { name: "Projects" }).click();
  await expect(dialog.getByText("Destination: Home/Projects")).toBeVisible();
  await dialog.getByRole("button", { name: "Move Here" }).click();

  await expect(dialog).toBeHidden();
  await expect(page.getByText("explora-notes.md")).toBeHidden();
  await page.getByText("Projects").dblclick();
  await expect(page.getByText("explora-notes.md")).toBeVisible();
});

test("trashes locally and confirms permanent deletion with platform shortcuts", async ({
  page,
}) => {
  await page.goto("/");
  const isMac = await page.evaluate(() =>
    /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent),
  );

  await page.getByText("explora-notes.md").click();
  await page.keyboard.press(isMac ? "Meta+Backspace" : "Delete");
  await expect(page.getByText("explora-notes.md")).toBeHidden();
  await expect(page.getByRole("dialog")).toBeHidden();

  await page.reload();
  await page.getByText("explora-notes.md").click();
  const permanentShortcut = isMac ? "Meta+Alt+Backspace" : "Shift+Delete";
  await page.keyboard.press(permanentShortcut);
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("heading", {
      name: "Delete “explora-notes.md” permanently?",
    }),
  ).toBeVisible();
  await expect(dialog.getByText("In Home")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
  await dialog.getByRole("button", { name: "Cancel" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText("explora-notes.md")).toBeVisible();

  await page.keyboard.press(permanentShortcut);
  await dialog.getByRole("button", { name: "Delete Permanently" }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText("explora-notes.md")).toBeHidden();
});

test("previews a multipage PDF with custom canvas controls", async ({
  page,
}) => {
  await page.goto("/");

  await page.getByText("handoff.pdf").click();
  await page.keyboard.press("Space");

  const dialog = page.getByRole("dialog");
  const document = page.getByRole("application", {
    name: "PDF preview of handoff.pdf",
  });
  await expect(document).toBeVisible();
  await expect(dialog.locator('[data-slot="dialog-header"]')).toHaveClass(
    /sr-only/,
  );
  await expect(dialog.getByText("1 / 3")).toBeVisible();
  await expect(
    dialog.getByRole("complementary", { name: "PDF pages" }),
  ).toBeVisible();
  await expect
    .poll(() =>
      dialog
        .locator("canvas")
        .evaluateAll((canvases) =>
          canvases.some(
            (canvas) =>
              (canvas as HTMLCanvasElement).width > 0 &&
              (canvas as HTMLCanvasElement).height > 0,
          ),
        ),
    )
    .toBe(true);

  await dialog.getByRole("button", { name: "Zoom in" }).click();
  await expect(dialog.getByText("125%")).toBeVisible();
  await dialog.getByRole("button", { name: "Hide page thumbnails" }).click();
  await expect(
    dialog.getByRole("complementary", { name: "PDF pages" }),
  ).toBeHidden();

  await document.focus();
  await page.keyboard.press("ArrowDown");
  await expect(document).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(dialog).toBeHidden();
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

test("opens a discovered volume from Locations", async ({ page }) => {
  await page.goto("/");

  await expect(page.getByText("Locations", { exact: true })).toBeVisible();
  const locations = page.getByRole("navigation", {
    name: "Mounted locations",
  });
  await locations.getByRole("button", { name: "Workspace" }).click();
  await expect(
    locations.getByRole("button", { name: "Workspace" }),
  ).toHaveAttribute("aria-current", "page");
});

test("configures standard favorites from the section header", async ({
  page,
}) => {
  await page.goto("/");
  const favorites = page.getByRole("navigation", { name: "Favorites" });

  await page.getByText("Favorites", { exact: true }).hover();
  await page.getByRole("button", { name: "Configure favorites" }).click();
  await favorites
    .getByRole("button", { name: "Remove Home from Favorites" })
    .click();
  await expect(
    favorites.getByRole("button", { name: "Home", exact: true }),
  ).toBeHidden();
  await expect(favorites.getByLabel("Home, not in Favorites")).toBeVisible();

  await favorites
    .getByRole("button", { name: "Add Home to Favorites" })
    .click();
  await expect(
    favorites.getByRole("button", { name: "Home", exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(
    page.getByRole("button", { name: "Configure favorites" }),
  ).toBeFocused();
});

test("configures visible SSH targets without disconnecting them", async ({
  page,
}) => {
  await page.goto("/");
  const sshTargets = page.getByRole("navigation", { name: "SSH targets" });

  await page.getByText("SSH", { exact: true }).hover();
  await page.getByRole("button", { name: "Configure SSH targets" }).click();
  await expect(
    page.getByRole("button", { name: "Finish editing SSH targets" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Add SSH target" }),
  ).toBeHidden();
  await sshTargets
    .getByRole("button", { name: "Hide staging-box from SSH" })
    .click();
  await expect(
    sshTargets.getByRole("button", {
      name: "staging-box connected",
      exact: true,
    }),
  ).toBeHidden();
  await expect(
    sshTargets.getByLabel("staging-box, hidden from SSH"),
  ).toBeVisible();

  await sshTargets
    .getByRole("button", { name: "Show staging-box in SSH" })
    .click();
  await page.keyboard.press("Escape");
  await expect(
    page.getByRole("button", { name: "Configure SSH targets" }),
  ).toBeFocused();
  await expect(sshTargets.getByRole("button", { name: /Manage / })).toHaveCount(
    0,
  );
  await expect(sshTargets.getByText("Config", { exact: true })).toHaveCount(0);

  await sshTargets
    .getByRole("button", { name: "staging-box connected", exact: true })
    .click({ button: "right" });
  await expect(page.getByRole("menuitem", { name: "Edit" })).toBeVisible();
  await expect(
    page.getByRole("menuitem", { name: "Disconnect" }),
  ).toBeVisible();
  await expect(page.getByRole("menuitem", { name: "Remove" })).toBeVisible();

  await page.keyboard.press("Escape");
  await sshTargets
    .getByRole("button", { name: "staging-box connected", exact: true })
    .focus();
  await page.keyboard.press("Shift+F10");
  await expect(page.getByRole("menuitem", { name: "Edit" })).toBeVisible();
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

  await page
    .getByRole("button", { name: "render-node disconnected", exact: true })
    .click();
  await expect(page.getByRole("tab", { name: "render-node" })).toBeVisible();
  await expect(page.getByText("This location is empty")).toBeVisible();
});

test("preserves an SSH tab through disconnect, reconnect, and refresh", async ({
  page,
}) => {
  await page.goto("/");

  const renderNode = page.getByRole("button", {
    name: "render-node disconnected",
    exact: true,
  });
  await renderNode.click();
  const remoteTab = page.getByRole("tab", { name: "render-node" });
  await expect(remoteTab).toBeVisible();

  await page
    .getByRole("button", { name: "render-node connected", exact: true })
    .click({ button: "right" });
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
